# Audit, signierte Historie und Insights in V2

Die neue Auditprojektion enthält ausschließlich Ereignismetadaten. Persönliche
Läufe, Konten und Organisationen besitzen getrennte SHA-256-Ketten mit eigenen
Ketten-UUIDs. Eine normale persönliche Owner-Verbindung erteilt keine
Organisationsberechtigung. Die Konto- und Organisations-APIs prüfen die aktuelle
globale Sitzung unter dem Identity-Writerlock und erneut nach dem Kettenlock;
Organisationshistorie verlangt eine aktive Owner-/Admin-Mitgliedschaft.

## Vorhandene Historie und Originalbytes

Die Migration friert die IDs vorhandener persönlicher Ereignisse und die letzte
Identity-Sequenz ein. Alte Ereignisse werden als `legacy_unsigned` in die neue
Projektion übernommen. Ihre alte SQL-Historie bleibt erhalten. Die Projektion
ordnet nach der Aufnahme in die neue Kette; `observed_at` bleibt die ursprüngliche
Zeit und ist kein Beweis einer ursprünglichen Signatur.

Ausgewählte V1-JSONL-Dateien, rotierte Ledgerdateien und sonstige Originalarchive
können einzeln archiviert werden: maximal 16 MiB pro Datei, 64 Dateien und ungefähr
256 MiB Originaldaten pro Scope. Originalbytes und eingebettete Signaturen werden
unverändert in der Datenbank erhalten und mit einem SHA-256-Anker an die neue
Kette gebunden. SQL-Backups nehmen diese Daten mit. Ein Download der Originalbytes
ist eine ausdrückliche Aktion derselben aktuell autorisierten Person.

Die ursprüngliche V1-Signatur ist HMAC-basiert und benutzt einen separaten nativen
Schlüssel. V2 behauptet ohne diesen Schlüssel keine Prüfung: `legacy_unverified`
ist ein Archivstatus, keine Ausführungsfreigabe. Ein V2-Archiv wird erst als
`verified_v2_signature` aufgenommen, wenn alle Ketten- und Ed25519-Prüfungen mit
dem ausdrücklich angegebenen öffentlichen Fingerprint bestehen. Der Fingerprint
muss über einen vertrauenswürdigen separaten Weg bestätigt werden. Originaldaten
werden weder als Konten, Sitzungsschlüssel noch als Laufrechte übernommen.

## Signierte Exporte und Offlineprüfung

Jede Archivseite besitzt höchstens 500 Metadateneinträge, eine eingefrorene
Endsequenz, Anfangs-/Endhash, Scope und Ketten-UUID. Ed25519 authentisiert die
gesamte Seite einschließlich Grenzen und öffentlichem Schlüssel. Der Signer wird
mit einem eigenen HMAC-Purpose aus dem bereits privat verwahrten Team-KeyStore
abgeleitet; der private Schlüssel gelangt niemals in den Renderer. Ein
öffentliches Archiv bleibt nach einer Schlüsseländerung mit seinem ursprünglichen
öffentlichen Schlüssel überprüfbar. Die automatische Vertrauensübernahme eines
neuen Fingerprints ist nicht vorgesehen.

Der Exportbutton speichert bis zu zehn signierte Seiten pro Abschnitt. Weitere
Abschnitte laufen bis zur eingefrorenen Endsequenz weiter, auch wenn neue
Ereignisse hinzugekommen sind. Jeder Abschnitt enthält selbstständig prüfbare
Seiten. Vollständigkeit gilt erst, wenn der erste Abschnitt bei Sequenz null
beginnt und sämtliche Abschnittsgrenzen ohne Lücke bis zum Endhash zusammenpassen.
Ein einzelner mittlerer Abschnitt ist eine signierte Teilhistorie. Eine
`request_id` liefert dauerhaft dasselbe Ergebnis; geänderte Parameter mit derselben
ID werden abgewiesen. Bis zu 4096 unveränderte Archivreceipts pro Scope bleiben
erhalten; keine automatische Löschung verkürzt die Historie.

Offline kann `tools/audit/verify_archive.py` mit Python und `cryptography` sowohl
eine einzelne Seite als auch einen UI-Abschnitt prüfen:

```powershell
python tools/audit/verify_archive.py selected-archive.json --public-key-sha256 <independently-confirmed-64-hex-fingerprint>
```

Der Prüfer arbeitet ohne Server oder Netzwerk und meldet ausschließlich Status,
Scope, Ketten-ID, Sequenzgrenzen und Fingerprint. Ein falscher Fingerprint, eine
veränderte Signatur, fremde Zusatzfelder, lückenhafte Sequenzen und unpassende
Abschnittsgrenzen werden abgewiesen. Eine gültige Signatur bestätigt den Inhalt
zum Archivzeitpunkt; die SQL-Kette allein ist kein Schutz gegen einen Angreifer,
der die gesamte Datenbank vor dem ersten signierten Export neu schreiben kann.

## Insights und redaktierte Diagnostik

Die UI zeigt Mengen, Laufzustände, die letzten Metadatenereignisse,
Aktionskategorien und Integritätsstatus. Persönliche Mengen werden aus dem
persönlichen Produktspeicher ermittelt; Team-Mengen aus dem ausgewählten Team.
Zusätzlich enthalten Insights unveränderliche, getrennt autorisierte
Ausführungsbelege: tatsächlich gemeldete Eingabe-/Ausgabetokens, gemessene
Anfrage- und Laufdauern, Kostenbelege und verwendete Skill-Kontexte. Die UI bietet
7/30/90/365 Tage und den gleich langen vorherigen Zeitraum. Beim Öffnen und
erneuten Anzeigen lädt die Übersicht automatisch; ein Zeitraumwechsel lädt
ebenfalls sofort. „Aktualisieren“ ruft den gewählten Zeitraum bis zum aktuellen
Zeitpunkt ab. Weitere Belegseiten behalten dagegen Zeitfenster und Snapshot
der angezeigten Auswertung. Die API akzeptiert
`from_ms`, `until_ms`, `after_sequence`, `snapshot_sequence` und `limit` (1–200).
Jede Belegseite bleibt auf derselben eingefrorenen Sequenz; Zeitfenster sind
halboffen, höchstens 366 Tage und nicht in der Serverzukunft. Beide Summen werden
über höchstens 100.000 hashgeprüfte Belege ermittelt; ein erreichtes Limit wird
ausdrücklich als unvollständig angezeigt.

Persönliche Belege stammen aus bestätigten Modellattempts und den ursprünglichen
Receive-/Preisreceipts. Teambelege stammen nur aus ursprünglichen Worker-ACKs;
manuelle Recoveryentscheidungen sind keine Anbieterbestätigung. Reservierte
Tokens, gehaltene Beträge und Katalogauswahl werden niemals als Verbrauch
gezählt. Providerkosten und Berechnungen mit eingefrorenen Preisen besitzen
getrennte Summen und Währungen; sie werden nicht als zwei Zahlungen addiert.
Providerbeträge werden ohne Rundung als Dezimaleinheiten gespeichert. Fehlende
Kosten oder Währung bleiben unbekannt und werden nicht durch eine USD-Annahme
oder eine Preisschätzung ersetzt. Anbieterangaben sind keine Rechnungsprüfung.

Skillnutzung bedeutet belegte Aufnahme der versionierten Skill-Anweisungen in
den tatsächlichen Modellkontext. Gespeichert werden nur IDs, Revisionen und
Hashes; Anweisungstexte gelangen nicht in diese Projektion. Für persönliche
Workflows wird der tatsächliche Agentturn geprüft. Ohne bestätigtes Resultat,
bei historisch fehlender Provenienz und bei rein unbestätigten unterbrochenen
Sendungen wird keine Skill-Ausführung erfunden. Unbekannte Dauer und
unvollständige Tokenangaben bleiben sichtbar. Alte Team-ACKs ohne Ausführungszeit
werden ausdrücklich unter dem unveränderten Erstellungszeitpunkt des Laufs
gruppiert; ihre Dauer und Skillprovenienz bleiben unbekannt.
Aktionskategorien beziehen sich ausdrücklich auf die letzten 10.000 Einträge.
Die vollständige Kettenprüfung verarbeitet höchstens 100.000 Einträge in
256er-Seiten; darüber lautet `complete=false`, niemals vollständig geprüft.

Diagnostikexporte enthalten keine Prompts, Modellantworten, Werkzeugargumente,
Zugangsdaten, Dateien, URLs oder native Pfade. Bei einem Kettenfehler werden keine
ungeprüften Ereignistexte in Insights oder Diagnostik ausgegeben. Die ursprüngliche
Historie wird weder gelöscht noch automatisch repariert. Ein Archivimport erzeugt
einen nachvollziehbaren Anker; eine beschädigte bestehende Kette muss gesondert
untersucht werden und wird nicht stillschweigend durch eine scheinbar gültige
Historie ersetzt.

API: `/api/v2/audit`, `/api/v2/insights`, `/api/v2/audit/archives`,
`/api/v2/audit/history`, `/api/v2/audit/verification`,
`/api/v2/audit/diagnostics`. Für Konten gelten die entsprechenden Unterpfade von
`/api/v2/identity/me/audit-v2`; für Teams von
`/api/v2/identity/organizations/{organization}/audit-v2`.

## Releasegrenzen

Der Quellcode und enge Prüfungen ersetzen keine Qualifikation eines veröffentlichten
Installers oder eines echten HA-Servers. Die unten beschriebene KMS-/Masterrotation
ist ein eigener Operatorablauf. V1-Prüfungen, explizite Archivierung
und unabhängigen Kontrollpunkte sind im Produktcode integriert; native
Bedienabläufe, veröffentlichte Installer und ein tatsächlich unabhängig
betriebenes Witness-System sind dadurch noch nicht qualifiziert. Keine
Diagnostik wird automatisch veröffentlicht; ein neuer Tokenbudgetdienst ist
nicht erforderlich.

## Originale V1-HMAC-Prüfung und beschädigte Ketten

Im persönlichen nativen Windows-Desktop können bis zu vier archivierte
Originaldateien in ausdrücklicher chronologischer Reihenfolge geprüft werden.
Die feste V1-Anmeldeinformation
`audit-signing-key-v1.com.open-cowork.desktop.audit.v1` wird mit `CredReadW`
nur gelesen. Weder Renderer noch API liefern Schlüsselbytes oder einen
frei wählbaren Keyring-Locator. Fehlende oder ungültige Originalschlüssel
werden nicht erzeugt, ersetzt oder aus einer anderen Ablage übernommen.
Bereits der manifestgebunden geprüfte V1-Quellstand v0.3.12 verweigert den
nativen Audit-Signierer außerhalb von Windows. Deshalb wird kein zusätzlicher
Linux-Keyring-Locator erfunden. Erhaltene Windows-Originaldateien können auf
Windows geprüft und anschließend mit ihrem signierten Prüfbeleg archiviert werden.
Eine native Bestätigung nennt Originalhashes und dieselbe Vorgangs-ID.
Vollständige ausgewählte Ketten, gültige Teilketten und unsignierte Ereignisse
bleiben getrennte Aussagen. Die Prüfung beweist nicht, dass sämtliche früheren
Ledgerdateien vorhanden sind. Ein eigener signierter Prüfbeleg bindet die
ausgewählten Originalhashes und das Ergebnis; der ursprüngliche Archivstatus
wird nicht rückwirkend aufgewertet.

Für eine nachweislich beschädigte V2-Projektion gibt es eine ausdrückliche
Vorschau und Bestätigung über `/audit/recovery`. Der SHA-256 des vollständigen
Originalabbilds ist eine CAS-Bedingung: Jede zwischenzeitliche Änderung
verlangt eine neue Vorschau. Originale SQL-Textwerte einschließlich fehlerhafter
JSON-Nutzlasten, Spaltenwerte und Kettenkopf werden atomar unverändert als
separates Abbild archiviert. Erst danach beginnt eine neue Ketten-UUID mit
einem signierten Herkunftsbeleg. Die alte Kette wird niemals als repariert
oder ursprünglich gültig bezeichnet. Originalexporte, frühere Signaturen,
Rohereignisse und ungeklärte Ausführungsbelege bleiben erhalten. Gleiche
Vorgangs-ID und gleiche Originalparameter liefern dauerhaft denselben Beleg;
geänderte Parameter werden abgewiesen. Pro Scope gelten acht Originalabbilder,
je höchstens 100.000 Zeilen/64 MiB und zusammen 512 MiB Base64-Daten. Kein
automatisches Löschen verschleiert ein erreichtes Aufbewahrungslimit.

## Unabhängiger Audit-Witness

`cowork-audit-witness` ist ein separates natives Windows-/Linux-Programm für
einen ausdrücklich bedienten Dateiaustausch. Sein Ed25519-Schlüssel und seine
unveränderlichen Herkunftspolicies, vollständigen Aufträge und Originalquittungen
liegen außerhalb der Produktdatenbank. Für einen unabhängigen Schutz wird es
auf einem anderen administrativen System beziehungsweise einem unabhängig
kontrollierten Datenträger betrieben. Ein zweiter Ordner unter derselben
Angreiferkontrolle schafft keine unabhängige Vertrauensgrenze.

Die Produktoberfläche bindet eine dauerhafte Herkunfts-UUID, den bestätigten
Origin-Fingerprint und den unabhängig bestätigten Witness-Fingerprint. Diese
Bindung ist signiert und unveränderlich. Der Witness autorisiert dieselbe
Herkunft nur mit einem separat übergebenen Origin-Fingerprint; eine Datei kann
ihren eingebetteten Schlüssel niemals selbst für vertrauenswürdig erklären.
Die Oberfläche merkt die ausdrücklich bestätigten Fingerprints getrennt von
der Serverdatenbank. Auf einem neuen Client müssen sie erneut ausdrücklich
bestätigt werden; geänderte Fingerprints werden nicht automatisch übernommen.
Der Witnesswechsel verwendet einen eigenen ausdrücklich bestätigten Übergang.
Ein ursprünglicher Auftrag bindet das bisherige und das neue signierte Binding
sowie exakt die letzte unabhängig bestätigte Quittung. Beide Origin- und beide
Witness-Signaturen werden geprüft; auch bei gleichem Origin sind beide
zweckgebundenen Signaturen vorhanden. Die ursprünglichen Bindings, Aufträge und
Originalquittungen bleiben unter ihren damaligen Schlüsseln lesbar. Ein Server-
oder Client-Pin wird nicht allein aus einer neuen Datei übernommen.

Die Oberfläche exportiert den signierten Übergangsauftrag, verlangt den separat
bestätigten neuen Witness-Fingerprint und bewahrt einen ausstehenden Import
einschließlich Originalbytes und bisheriger Pins für Antwortverlust. Ein offener
Übergangsauftrag sperrt neue Kontrollpunkt-Captures; er wird mit derselben ID
fortgesetzt. Die Binding-Aktivierung, historische Originalbindung und der
64-KiB-Originalimport werden atomar per SQL-CAS veröffentlicht. Bis zu 128
Übergänge pro Scope bleiben erhalten. Ein neuer Client bestätigt die
ursprünglichen Root-Fingerprints und prüft daraus die vollständig signierte
Fortsetzung; die aktuelle SQL-Bindung allein schafft keinen neuen Root.

Der native Befehl `handover` benutzt zwei ausdrücklich initialisierte private
Witness-States. Er prüft den bisherigen State und dessen tatsächlich letzte
Originalquittung. Er publiziert zuerst einen vorbereiteten Beleg im neuen State,
dann die dauerhafte Signatursperre für das bisherige Binding im alten State,
erst danach den aktivierten Übergang im neuen State. Ein noch vorbereiteter
Empfänger kann keine Fortsetzung signieren. Jede verlorene Bestätigung wird
aus denselben ursprünglichen Dateien abgeglichen; keine Signierseed- oder
Headerdatei wird ersetzt. Der bisherige State liefert weiterhin exakt bekannte
Originalreceipts, verweigert aber neue Signaturen für das übergebene Binding.
Der neue State übernimmt die alte Sequenz und denselben externen Hashanker.
Das 512-Quittungen-Limit gilt auch nach einem Handover weiter.

Der serverseitige Audit-Origin-Wechsel verwendet den getrennten privaten
Operatorablauf unten. Der ursprüngliche Operator-Signierschlüssel bleibt dabei
unverändert. Die Oberfläche wechselt den Witness unter demselben Origin und
prüft danach auch die vom Operator importierten Cross-Origin-Belege. Ein echter
nativer Handover zwischen produktiv betriebenen Witness-States wurde hier nicht
abgenommen.

Ein Kontrollpunktauftrag friert den aktuellen vollständig geprüften Kettenkopf
mit Scope, Ketten-UUID, Ursprung, Vorgangs-ID und vorherigem Kontrollpunkt ein.
Er enthält einen vollständigen signierten Metadatenbeleg von Sequenz null bis
zum eingefrorenen Kopf, höchstens 100.000 Ereignisse/64 MiB. Ein bereits
angelegter Auftrag bleibt auch nach Antwortverlust oder weiteren Ereignissen
derselbe. Pro Scope darf ein ursprünglicher Auftrag gleichzeitig offen sein;
vor dem nächsten wird sein tatsächlicher Originalbeleg importiert. Es gibt
keine automatische Netzübertragung, Wiederholung unbekannter Sendungen oder
Vertrauensübernahme.

Der Witness prüft vollständige Origin-Signaturen, alle Sequenzen und Hashes
sowie seine separat aufbewahrte letzte Quittung. Er verweigert Rücksprünge,
veränderte Präfixe und einen fehlenden oder fremden Vorgänger. Auch eine
vollständig neu berechnete und mit dem gültigen Origin-Schlüssel neu signierte
Datenbankgeschichte muss den zuvor extern bestätigten Hash enthalten. Ein
erster Kontrollpunkt bestätigt ausschließlich den vorgelegten Anfangsbestand;
er beweist keine zuvor fehlenden Daten. Ein Kettenwechsel wird nur mit der
genauen neuen Ketten-UUID als ausdrücklicher unabhängiger Operatorentscheidung
zugelassen und bleibt in der signierten Quittung sichtbar.

Beispiel auf dem separaten Witness, mit absoluten operatorgewählten Pfaden:

```powershell
cowork-audit-witness init --state C:\AuditWitness\origin-store
cowork-audit-witness status --state C:\AuditWitness\origin-store
cowork-audit-witness authorize --state C:\AuditWitness\origin-store --binding C:\Exchange\origin-binding.json --origin-key-sha256 <separately-confirmed-origin-sha256>
cowork-audit-witness sign --state C:\AuditWitness\origin-store --request C:\Exchange\checkpoint-request.json --output C:\Exchange\new-checkpoint-receipt.json
cowork-audit-witness verify --request C:\Exchange\checkpoint-request.json --receipt C:\Exchange\new-checkpoint-receipt.json --origin-key-sha256 <separately-confirmed-origin-sha256> --witness-key-sha256 <separately-confirmed-witness-sha256>
# Nach einem zuvor importierten Originalkontrollpunkt und einem neuen UI-Auftrag:
cowork-audit-witness init --state C:\AuditWitness\next-origin-store
cowork-audit-witness handover --state C:\AuditWitness\origin-store --new-state C:\AuditWitness\next-origin-store --proposal C:\Exchange\original-handover-proposal.json --origin-key-sha256 <separately-confirmed-original-origin-sha256> --new-origin-key-sha256 <same-origin-sha256> --new-witness-key-sha256 <separately-confirmed-next-witness-sha256> --output C:\Exchange\original-handover-receipt.json
```

`init` verlangt einen neuen absoluten Zustandsordner und erzeugt den Schlüssel
mit einem kryptografischen Zufallsgenerator direkt unter privatem OS-Zugriff.
Ein Zustandskopf, die originale Schlüsselidentität und ein echter exklusiver
OS-Dateilock verhindern das versehentliche Starten mit einem Ersatzschlüssel
oder einem parallelen Writer. Originalbelege werden mit `CREATE_NEW`, Flush
und atomarer No-replace-Publikation geschrieben. Symlinks, Reparsepunkte,
Hardlinks, nicht private Schlüsseldateien und geänderte Eingabedateien werden
abgewiesen. Verwaiste temporäre Dateien werden nicht automatisch übernommen
oder gelöscht. Der Witness behält höchstens 512 Aufträge pro Herkunft/Scope,
8192 Dateien und 2 GiB Gesamtzustand. Diese Grenzen werden ausdrücklich
abgewiesen; er kürzt keine Belegkette.

Nach einem bewusst bestätigten Kettenwechsel ergänzt der unabhängige Operator
bei `sign` den Parameter `--permit-new-chain <exact-new-chain-uuid>`.
Ein Wiederholungsaufruf derselben Originalanfrage liefert die ursprüngliche
Quittung, ohne erneut zu signieren. Ein verlorener Ausgabedownload kann mit
demselben Auftrag und einem neuen Ausgabepfad wiederholt werden. Produktseitig
wird der 16-KiB-Originalbeleg hashgeprüft, mit dem gefrorenen ursprünglichen
Auftrag verglichen und unverändert aufbewahrt. Aktueller Präfixabgleich gilt
nur bei vollständig gültiger aktueller Produktkette. Originalquittungen müssen
weiter extern gesichert werden; ihre Kopie in der Produktdatenbank ersetzt
den unabhängigen Witnesszustand nicht.

Die APIs liegen unter `/audit/checkpoints` (`GET` Status, `POST` Auftrag),
`/witness`, `/import`, `/history`, `/requests/{request}` und
`/originals/{request}`. Die zusätzlichen Übergangspfade sind `/transitions`
(`POST` ursprünglicher Auftrag), `/transitions/{request}` (`GET` denselben
Auftrag), `/transitions/import` und `/transitions/{request}/original`. Konto- und Organisationsscopes verwenden dieselben
Unterpfade ihrer oben beschriebenen Identity-Auditbasis; aktuelle globale
Sitzung und Owner-/Admin-Rechte werden vor und nach dem Audit-Writerlock
erneut geprüft. Alle Kontrollpunkte sind reine Historie und erteilen keine
Ausführungs- oder Inhaltsrechte.

Ein eigenständiges natives Lieferarchiv entsteht explizit mit
`python tools/audit/package_witness.py --build --cargo <absolute-cargo> --output <new-absolute-zip-or-tar-gz>`.
Es enthält das native Programm, Dokumentation, Lizenz, Prüfsummen und eine
SPDX-SBOM. Schlüssel, Witnesszustand und Zugangsdaten sind niemals enthalten.
Das Paketprogramm installiert, startet, signiert oder veröffentlicht nichts.

Enge Quellprüfungen am 12.10.2026 belegen V1-Segmentprüfung, CAS-gebundene
Archivierung mit unveränderten fehlerhaften SQL-Bytes und erhaltenen früheren
Exportquittungen sowie die Verweigerung eines gültig neu Origin-signierten
Forks durch den unabhängigen Witness. Das sind gezielte SQLite-/Ring-Prüfungen,
keine Abnahme eines tatsächlich unabhängigen Produktionssystems.

## Server-KMS und Masterrotation

Der Server besitzt eine dauerhafte Registry für seine ursprüngliche Installation,
aktiven Datenschlüssel und getrennten Audit-, Offline- und Request-Fingerprint-
Referenzen. Die originale Identität des bisherigen Team-Masterschlüssels bleibt
erhalten; sein ursprünglicher hexadezimaler Dateitext wird nicht in eine neue
Schlüssel-ID umgerechnet. Die separate Desktop-NativeKeyStore-Ablage, V1-HMAC,
Identity-Bootstrap-/Settlementdateien, S3-Zugänge und Runner-Operatoridentitäten
gehören nicht zu dieser Rotation und werden dabei nicht übernommen.

`cowork-daemon server-keys` ist ein ausdrücklicher lokaler Operatorablauf vor
API und Workern. `initialize` registriert den bewiesenen Originalschlüssel und
zeigt Registry-SHA-256 sowie ursprünglichen Audit-Fingerprint; `status` zeigt
den unveränderten registrierten Stand und seinen Hash. Eine bestehende Registry
oder erhaltene Cipher-/Signaturdaten erlauben keinen Ersatz für einen fehlenden
Originalschlüssel. Initialisierung einer bereits auf KMS umgestellten Registry
wird abgewiesen, bevor eine neue Klartextdatei entstehen könnte.

Der tatsächliche Adapter nutzt die dokumentierte
[Vault-Transit-API](https://developer.hashicorp.com/vault/api-docs/secret/transit)
über einen ausdrücklich konfigurierten HTTPS-Ursprung. Er prüft einen bestehenden
abgeleiteten `aes256-gcm96`-Schlüssel, deaktiviertes Encrypt-Upsert sowie fehlende
Export-/Plaintextbackup-/Delete-/Convergent-Freigaben. Die Anwendung legt keinen
Vault-Schlüssel an, rotiert keine Cloudpolicy und übernimmt kein automatisch
gefundenes OS-Secret. Operatorseitig müssen Transit-Mount und Schlüssel diese
Policy bereits besitzen. Der Token benötigt die tatsächlich verwendeten
Read-, Encrypt-, Decrypt- und Rewrap-Rechte einschließlich Mountkonfiguration.

Eine private JSON-Datei, Token und optionale CA-Datei liegen innerhalb des
geschützten Datenordners, außerhalb des Webordners, unter absoluten Pfaden ohne
Links oder Hardlinks. Tokenbytes gelangen nicht in UI, Registry oder Quittungen.
Die JSON-Datei enthält `format:1`, `profile` mit `format:1`, einer festen
`provider_id`-UUID, `endpoint` als kanonischem HTTPS-Ursprung mit abschließendem
`/`, `mount`, `key_name` und optionaler `namespace`, außerdem `token_file`,
`ca_certificate_file` (Pfad oder `null`), `wrapping_version` und den separat
bewahrten `expected_registry_sha256` sowie den beim ursprünglichen `initialize`
bestätigten `expected_operator_public_key_sha256`. Beide Pins kommen aus der
privaten Operator-Datei; auch ein Originalreceipt-Replay übernimmt keinen
eingebetteten oder neu aus SQL gelieferten Signierschlüssel. Beim Serverstart benennt
`COWORK_SERVER_KMS_CONFIG_FILE` genau diese Datei. Für CLI-Vorgänge wird sie
ausdrücklich mit `--config` übergeben.

Beispiel eines ursprünglichen Auftrags nach `initialize`, mit selbst gewählten
absoluten Pfaden und UUIDs:

```powershell
cowork-daemon server-keys import-kms --request <new-operation-uuid> --confirm-instance <original-instance-uuid> --expected-revision 1 --config C:\CoworkData\server-kms.json
cowork-daemon server-keys rotate --request <new-operation-uuid> --confirm-instance <original-instance-uuid> --expected-revision <current-revision> --config C:\CoworkData\server-kms.json
cowork-daemon server-keys rewrap --request <new-operation-uuid> --confirm-instance <original-instance-uuid> --expected-revision <current-revision> --config C:\CoworkData\server-kms.json
cowork-daemon server-keys reconcile --request <original-operation-uuid> --confirm-instance <original-instance-uuid> --config C:\CoworkData\server-kms.json
cowork-daemon server-keys receipt --request <original-operation-uuid> --confirm-instance <original-instance-uuid> --origin-key-sha256 <independently-retained-original-operator-pin>
```

`import-kms` verwahrt denselben ursprünglichen Seed unter Vault. `rotate`
reserviert einen neuen Zufallsschlüssel mit derselben Installations-UUID;
vor dem ersten Provideraufruf bleiben sein Beweis, seine feste Envelope-UUID und
seine unter dem aktuellen Schlüssel verschlüsselten Originalbytes im SQL-Journal.
Eine verlorene Antwort erzeugt deshalb keinen Ersatzseed. `rewrap` verlangt eine
ausdrücklich höhere Vault-Version und ändert allein KMS-Hüllen, niemals Seeds,
Schlüssel-IDs oder Anwendungsquittungen. Alle erhaltenen Schlüssel werden durch
echtes Decrypt und ihren ursprünglichen Bestätigungstag geprüft. Keine Antwort
aktiviert sich selbst. Private Konfiguration und SQL-Originalvorgang werden nach
den Providerwartezeiten erneut geprüft; Redirects, Proxyübernahme und automatische
Retries sind abgeschaltet.
Der ursprüngliche Intent und die bestätigten ursprünglichen KMS-Hüllen besitzen
eigene zweckgebundene MACs. Seedbeweis, Envelope-ID und Quittungsbytes können so
nicht bei einer späteren Reconciliation aus SQL ersetzt werden. Nach bestätigter
KMS-Verwahrung wird die provisorische, unter dem alten Master geschützte Seedhülle
aus dem aktiven Journal entfernt; ihr authentisierter Originaldigest bleibt
erhalten. Frühere SQL/WAL-/Dateibackups können ältere Schlüsselstände enthalten
und müssen getrennt geschützt werden; die Rotation behauptet keine Löschung
solcher früheren Kopien.

Während eines offenen Vorgangs verweigert der gemeinsame SQL-Writerguard neue
verschlüsselte Serverwrites und neue private Serverclaims. Eine Datenschlüssel-
Rotation reauthentisiert und reverschlüsselt Team-LiteLLM, Team-MCP-Konfigurationen
und authentische Session-Checkpoints, GitHub, persönliche Provider, Android-
Provider, Snapshotchunks, private Loop-Checkpoints/Intents/Receipts/Dateiversionen,
private MCP-Bindings/Kataloge/Session-Checkpoints, Push-Abonnements, TOTP-Geheimnisse,
verschlüsselte ursprüngliche Auth-Vorgänge und Operator-Geräteeinstellungen. Alle Änderungen verwenden ihre ursprüngliche AAD
und Originaltext-CAS in derselben SQL-Transaktion wie die Aktivierung. Geschäfts-
revisionen, Plaintextbytes, Receipt-/Request-MACs, Deduplizierung und ursprüngliche
Signaturen bleiben erhalten. Das gilt auch für separat bewahrte MCP-MAC-Schlüssel.
Native Cipher eines anderen Instanzbereichs werden nicht übernommen.

Laufende beziehungsweise ungeklärte Ausführungen sperren den Vorgang; eine
abgelaufene Lease ersetzt keinen Abschlussnachweis. Ein Fehler rollt sämtliche
Cipheränderungen zurück und lässt den Originalauftrag für die ausdrückliche
Reconciliation erhalten. Alte Runtime-Snapshots können nach Aktivierung nicht
mehr unter dem alten Datenschlüssel veröffentlichen. Der Server lädt bewiesene
Schlüssel nur in `Zeroizing`-RAM, vor der Zulassung von Workern. KMS-Revoke wirkt
beim Verwerfen/Neuladen dieses Snapshots; eine sofortige Vernichtung bereits
geladener RAM-Schlüssel wird nicht behauptet.
Dasselbe gilt für jeden registrierten privaten Container, bis sein eigener
Originalvorgang `removed` bestätigt; beendete Prozesse oder abgelaufene Runs
ersetzen diesen Nachweis nicht. Vor dem Laden der Cipherzeilen prüft die
Transaktion höchstens 100.000 Zeilen und zusammen 512 MiB gespeicherte Cipherbytes.

Jeder akzeptierte Vorgang erhält eine dauerhafte Ed25519-Quittung unter dem
ursprünglichen Operator-Signer. Vor dem ersten Origin-Wechsel ist dies derselbe
ursprüngliche Audit-Schlüssel; danach bleibt dieser Operator-Slot ausdrücklich
getrennt erhalten. Gleiche Vorgangs-ID und Parameter liefern dieselbe
Originalquittung. Nach Abschluss oder explizitem `cancel` eines noch nicht
aktivierten KMS-Vorgangs muss der Operator den privaten Registry-Pin auf den
signierten `result_registry_sha256` aktualisieren und alte Runtime-Prozesse
geordnet neu starten. Bis dahin verweigert der Serverstart einen abweichenden
Stand. Dadurch wird ein aus SQL geladener älterer Schlüsselstand nicht automatisch
für aktuell erklärt. Der ursprüngliche Audit-Fingerprint und alle bestehenden
externen Witnessbindungen bleiben bei diesen Daten-/KMS-Rotationen gleich.
512 Vorgänge und 1024 behaltene Schlüssel sind explizite Grenzen ohne automatische
Receipt-/Keylöschung; eine Cipherrotation verarbeitet höchstens 100.000 Änderungen.

### Privater Audit-Origin-Wechsel

`rotate-audit` reserviert einen neuen Signierseed im bestehenden ausdrücklich
konfigurierten KMS, mit demselben Originaljournal wie die Datenrotation. Es ändert
keinen Datenschlüssel. Vor der Vorbereitung müssen sämtliche alten externen
Kontrollpunkte bereits importiert sein; jedes aktuelle Witness-Binding braucht
seinen tatsächlich letzten Beleg. Fremde offene Binding-Übergänge sperren den
Auftrag. Während der Vorbereitung verweigert die Signatur-Epoche alle frischen
Audit-Signaturen. Exakte Originalaufträge bleiben unverändert abrufbar.

Der ausgegebene öffentliche Plan enthält beide Origin-Signaturen, die originale
Registry, beide Schlüsselreferenzen und für jedes gebundene Scope das bisherige
Binding samt letztem Kontrollpunkt und dem neuen Binding. Diese all-scope Datei
wird ausschließlich dem lokalen Operator ausgegeben, nicht einer Tenant-API.
`reconcile` liefert denselben Plan mit denselben IDs. `audit-plan` prüft seine
beiden unabhängig bewahrten Origin-Pins; nach Abschluss außerdem die originale
Operatorquittung und deren Plan-Digest.

Auf jedem unabhängig betriebenen Witness prüft `handover-plan` den gesamten
Plan gegen beide separat übergebenen Origin-Pins und wählt nur das ausdrücklich
benannte Scope. Bei reinem Origin-Wechsel darf `--new-state` dieselbe private
Witnessablage bezeichnen. Die vorhandene native Prepare/Out/In-Folge bleibt
maßgeblich; der Witness-Signierseed wird nicht ersetzt. Jeder ausgegebene
Originalbeleg wird getrennt bewahrt. `bundle-approvals` liest eine JSON-Liste
absoluter Pfade zu diesen Originalbelegen und schreibt ein neues JSON-Bündel mit
deren exakten Base64-Bytes; es übernimmt daraus keinen Vertrauensanker.

```powershell
cowork-daemon server-keys rotate-audit --request <new-operation-uuid> --confirm-instance <original-instance-uuid> --expected-revision <current-revision> --config C:\CoworkData\server-kms.json
cowork-audit-witness handover-plan --state C:\AuditWitness\origin-store --new-state C:\AuditWitness\origin-store --plan C:\Exchange\original-origin-plan.json --scope personal --origin-key-sha256 <independent-old-origin-pin> --new-origin-key-sha256 <independent-new-origin-pin> --new-witness-key-sha256 <independent-current-witness-pin> --output C:\Exchange\original-origin-approval.json
cowork-audit-witness bundle-approvals --files-json C:\Exchange\approval-paths.json --output C:\Exchange\original-approvals.json
cowork-daemon server-keys activate-audit --request <original-operation-uuid> --confirm-instance <original-instance-uuid> --config C:\CoworkData\server-kms.json --confirm-origin-key-sha256 <independent-new-origin-pin> --audit-approvals-file C:\CoworkData\original-approvals.json
cowork-daemon server-keys audit-plan --request <original-operation-uuid> --confirm-instance <original-instance-uuid> --origin-key-sha256 <independent-old-origin-pin> --new-origin-key-sha256 <independent-new-origin-pin> --operator-key-sha256 <independently-retained-original-operator-pin>
```

Die Aktivierung verlangt genau einen originalen Witness-Beleg je geplantem
Binding. Beide Witness-Signaturen, die bisherigen Bindings und Kontrollpunkte
werden geprüft. Die Signatur-Epoche, alle Binding-CAS, die historischen
Originalbytes, die Registry und die Operatorquittung werden in derselben
SQL-Transaktion aktiviert. Daten-Cipher, Offline-Signer, Request-Fingerprints,
alte Auditarchive und alte Operatorquittungen werden dabei nicht umgeschrieben.
Die neue Runtime muss ausdrücklich den bestätigten Registry-Pin und die neue
Epoche laden. Ein alter Runtime-Signer kann auch bei gleichem Datenschlüssel
keine frische Signatur veröffentlichen.

Normale Captures halten Scoped-Head, dauerhaften Signaturguard und Signatur-Epoche
bis zur Veröffentlichung. Der Guard existiert bereits vor der ersten Epoch-Zeile.
PostgreSQL verwendet echte `FOR SHARE`-Sperren gemäß der
[PostgreSQL-Sperrdokumentation](https://www.postgresql.org/docs/current/explicit-locking.html#LOCKING-ROWS).
Der private Wechsel sperrt alle vorhandenen Heads geordnet, dann exklusiv Guard
und Epoche und vergleicht
nochmals die vollständige Head-Menge. Ein gleichzeitig neu angelegtes Scope
kann dadurch nicht unbemerkt am Wechsel vorbeigehen. Die Erhaltung unabhängiger
Witnesszustände und separat bewahrter Pins bleibt Voraussetzung gegen einen
vollständigen manipulierten SQL-Rollback.

Nach Veröffentlichung des Plans ist `cancel` gesperrt: Ein externer Witness
könnte den alten Signierpfad schon dauerhaft gesperrt haben. Antwortverlust wird
mit derselben originalen Operation, demselben Plan und denselben Proofbytes
abgeglichen; unbekannte externe Effekte werden nicht zurückgesetzt. Vor einer
Planveröffentlichung kann der reine reservierte Vorgang ausdrücklich abbrechen.
Die aktuelle enge SQLite-Prüfung besteht: fehlerhafte Pins/Proofs rollen die
Aktivierung vollständig zurück; ursprüngliche Archive/Operatorbelege bleiben
identisch, der alte Runtime-Signer wird gesperrt und der neue kann fortsetzen.
Die native Witness-CLI kompiliert. Die gemeinsame aktuelle Daemon-Kompilierung
und eine getrennte physische Betriebsabnahme bleiben erforderlich.

Die bisherige Klartext-Masterdatei bleibt nach `import-kms` zunächst erhalten.
Erst ein eigener `retire-legacy-file`-Auftrag mit denselben vier Bindungsparametern
wie `rotate` beweist ihre ursprünglichen Keybytes, ihren Originaldateihash und
ihre OS-Dateiidentität, journalisiert die Absicht und entfernt ausschließlich
`team-keys/master.key` unter nativen Datei- und SQL-Fences. Dafür müssen Prozesse,
die den alten Dateikey halten, zuvor geordnet beendet sein. Der Bestätigungstag
aus dem KMS muss weiterhin stimmen. Nach unbestätigtem Dateieffekt wird derselbe
Auftrag ausdrücklich abgeglichen; ein fremder Ersatz wird nicht gelöscht oder
übernommen. Dieser Vorgang ist nicht automatisch abbrechbar und benötigt bei
Antwortverlust `reconcile`. Seine Quittung attestiert die beobachtete Abwesenheit
der Originaldatei; sie behauptet keine sichere Löschung aus SSD oder Backups.
Auch danach gibt es keinen Klartext-Fallback und keine Signer-Neuanlage.

Dies sind installierbare Operatorfunktionen im Quellcode. Ein echter Vault,
produktive Schlüsselmigration, unabhängige Live-Witness-Infrastruktur und
PostgreSQL-/HA-Ausfallqualifikation wurden hier nicht betrieben oder abgenommen.
