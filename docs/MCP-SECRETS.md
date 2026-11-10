# Native MCP-Schlüssel und verschlüsselte Zugangsdaten

Stand: 8. Oktober 2026. Der private Runtime-Einstieg
`dispatch_custodied_mcp_tool` löst die ausgewählten Zugangsdaten und den ursprünglichen
Sitzungsschlüssel aus einem vertrauenswürdigen Schlüsselspeicher auf. Er ergänzt den
vorhandenen Journal- und Sitzungsdienst. Die dauerhafte Schlüsseleinrichtung samt
nativer API und automatischer Auflösung der gespeicherten Auswahl sind in
[NATIVE-SECRET-SETUP.md](NATIVE-SECRET-SETUP.md) beschrieben. Der Desktop besitzt eine
Einrichtungsoberfläche mit nativer Bestätigung. Interaktive Betriebssystemabnahme,
Server-KMS und vollständige MCP-Einrichtung bleiben offen. Modellschleife und
Katalogimport/Auswahl sind angeschlossen; siehe [MCP-PRODUCT.md](MCP-PRODUCT.md).

## Schlüsselbesitz

`cowork-secrets` besitzt die Plattformadapter. Ein Schlüssel hat eine nicht geheime
Instanzkennung und eine zufällige Schlüsselkennung. Das Öffnen eines Speichers
erzeugt keinen Schlüssel. Nur der ausdrücklich aufgerufene Provisionierungsvorgang
erzeugt 256 zufällige Bits, speichert einen neuen Eintrag und liest ihn zur Kontrolle
zurück. Ein vorhandener Eintrag wird dabei nicht übernommen oder ersetzt.

Unter Windows werden `CredWriteW`, `CredReadW` und `CredDeleteW` mit einem eigenen
V2-Namensraum und `CRED_PERSIST_LOCAL_MACHINE` verwendet. Der Schlüssel bleibt an
das aktuelle Benutzerkonto auf diesem Rechner gebunden. Die Windows-Implementierung
von `keyring 3.6.3` verwendet dagegen `CRED_PERSIST_ENTERPRISE`; deshalb wird sie
hier nicht eingesetzt. Die Bedeutung der Persistenzwerte ist im
[Windows-Credential-Vertrag](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw)
beschrieben. Beim Lesen werden Typ, Persistenz, Format, Länge und Instanz-/Schlüsselkennung
geprüft. Der von Windows zurückgegebene Geheimnispuffer wird vor `CredFree` geleert.

Unter Linux wird ausdrücklich der Secret-Service-Adapter aus `keyring 3.6.3`
gewählt, mit synchronem DBus-Zugriff und verschlüsselter Übertragung über `crypto-rust`.
Es gibt keinen globalen Standardadapter und keinen Rückfall auf den Mock-Speicher,
Kernel-Keyring, eine Datei oder eine Umgebungsvariable. Ein verfügbarer Secret Service
ist eine Voraussetzung. Die Backendauswahl folgt der
[Dokumentation der verwendeten Bibliotheksversion](https://docs.rs/keyring/3.6.3/keyring/).

Native Aufrufe werden innerhalb des Prozesses serialisiert. Die Runtime führt sie
auf blockierenden Hilfsthreads aus, begrenzt parallel laufende Auflösungen auf zwei
und wartet einschließlich der Slotvergabe maximal zehn Sekunden. Ein bereits
laufender OS-/DBus-Aufruf kann diesen Zeitraum überschreiten; er behält seinen Slot
bis zum tatsächlichen Ende und führt anschließend keinen MCP-Aufruf aus.

## Verschlüsselte Credentials

Der native Speicher enthält ausschließlich die kleinen zufälligen Schlüssel.
Bearer-Token bis 4096 ASCII-Bytes werden in der Datenbank verschlüsselt aufbewahrt.
Ein öffentlicher Profilverweis enthält nur Referenz und Revision.

Die authentifizierte Bindung enthält:

- Projekt und MCP-Server;
- exakten normalisierten Endpoint;
- Credential-Referenz und Revision;
- Instanz-/Schlüsselkennung sowie Format und Verwendungszweck.

Das Format verwendet HKDF-SHA-256 und ChaCha20-Poly1305 aus `ring`. Ein zufälliger
256-Bit-Salt leitet für jede Verschlüsselung einen eigenen Schlüssel ab; dessen
Nonce wird genau einmal verwendet. Änderungen an Bindung, Schlüsselkennung,
Salt oder Ciphertext verhindern die Entschlüsselung. Schlüssel und temporäre
Klartextpuffer verwenden `Zeroizing`; vollständige Löschung aller Bibliotheks- und
Betriebssystemkopien wird damit nicht zugesichert.

Migration 14 ergänzt `mcp_credentials`. Änderungen benötigen die erwartete Revision.
Ein identischer erneuter Speichervorgang gibt denselben Datensatz zurück; ein
abweichender Ciphertext oder veralteter Stand erzeugt einen Konflikt. Nach einem
unklaren Datenbankabschluss muss deshalb der bereits erzeugte Ciphertext erhalten
und abgeglichen werden. Erneutes Verschlüsseln ist kein identischer Schreibversuch.

Rotation ersetzt den aktuellen Credential-Stand mit einer neuen Revision. Läufe,
deren eingefrorene Serverauswahl die alte Revision verlangt, erhalten dadurch keine
Zulassung mehr. Ein anderer Endpoint oder Server benötigt eine neue Referenz.
Entzug ist für die betreffende Referenz endgültig und entfernt den aktuellen
Ciphertext; ein erneutes Verbinden benötigt eine neue Referenz. Ältere Backups
können weiterhin frühere Ciphertexte enthalten. Dies ist keine Aussage über
physisches Löschen in WAL, Sicherungen oder Speichermedien.

## Auflösung und Ausführung

Vor der Geheimnisauflösung prüft der Runtime-Dienst Lease, Laufzustand, konkrete
Aktion, Policy/Freigabe und eingefrorene Serverauswahl. Ein bereits zugelassener
Werkzeugversuch wird aus dem Journal gelesen, ohne erneut Geheimnisse oder eine
Verbindung anzufordern.

Neue Sitzungen verwenden den ausdrücklich ausgewählten aktiven Schlüssel. Der Einstieg
`dispatch_configured_mcp_tool` liest dafür die dauerhaft bestätigte native Auswahl.
Bestehende Sitzungen verlangen ihren ursprünglichen Schlüssel aus dem aktuellen
Checkpoint, auch wenn sich die aktive Schlüsselwahl inzwischen geändert hat.
Ein fehlender Schlüssel erzeugt weder einen Ersatzschlüssel noch eine neue Sitzung.
Fehlende, entzogene, beschädigte oder falsch gebundene Credentials führen nicht zu
einer anonymen Verbindung oder einem anderen Profil.

Credential-Revision und Entzug werden in derselben Transaktion wie die
Werkzeugzulassung und nochmals vor dem Senden geprüft. Die Credential-Zeile bleibt
während der Zulassung gegen konkurrierende Rotation/Entzug gesperrt. Ein bereits
zugelassener entfernter Vorgang lässt sich dadurch nicht rückwirkend zurücknehmen;
ein Verbindungsabbruch ist weiterhin kein Nachweis seiner Beendigung.

Auftragsexporte, Ereignisse und öffentliche Laufdaten enthalten keine Schlüssel,
Token oder Credential-Ciphertexte. Eine vollständige Datenbanksicherung enthält
verschlüsselte Credential-Datensätze und benötigt die zugehörigen Schlüssel zur
Wiederherstellung.

## Prüfungen und noch offene Arbeit

Sieben neue reguläre Prüfungen decken Verschlüsselung/Bindung, Credential-Speicherung,
Revisionen, endgültigen Entzug und Runtime-Auflösung ab. Der Speichervertrag wird
zusätzlich gegen PostgreSQL geprüft. Der reale native Prozessprüfer provisioniert
ausschließlich einen neuen Testschlüssel, beendet den erzeugenden Kindprozess,
entschlüsselt danach eine synthetische Credential, löscht genau diesen Testschlüssel
und prüft den fehlgeschlagenen Wiederzugriff. Linux verwendet dafür einen separaten
DBus und ein neues temporäres GNOME-Keyring-Profil. Aktuelle Ergebnisse stehen in
[VERIFY.md](VERIFY.md).

Noch offen sind die interaktive Abnahme der nativen Einrichtungs-/Rotationsdialoge,
verschlüsselte Schlüsselsicherung und Wiederherstellung, sichere Bereinigung
verwaister Schlüssel nach unbestätigter Provisionierung und ein realer Server-KMS-Adapter.
Das Löschen eines Schlüssels verlangt eine gesonderte Prüfung aller abhängigen Daten;
der private native Adapter kann diese Datenbankbeziehungen nicht selbst prüfen.
Es gibt noch keine automatische Masterkey-Umschlüsselung bestehender Datensätze.
Ein Datenbank-Rollback auf einen früheren authentischen Stand und ein kompromittierter
OS-Benutzer werden durch die Verschlüsselung nicht verhindert.

OAuth, Secret-Service-Dialoge in realen Desktop-Sitzungen, Credential-Transfer bei
Migration, vollständige Backup-/Restore-Abnahme und Enterprise-Rollen sind noch offen.
Die bisherigen modellseitigen Provider-Credentials sind noch nicht auf diesen Dienst
umgestellt. Es wurde kein persönlicher Zugangsschlüssel verwendet.
