# Verschlüsselte MCP-Sitzungen

Stand: 8. Oktober 2026. Der private Runtime-Dienst kann eine bestätigte MCP-Sitzung
nach Freigabepausen, Worker-Wechsel und Prozessneustart weiterverwenden. Er öffnet
dafür den aktuellen verschlüsselten Checkpoint. Die Modellschleife und eine
Oberfläche für Katalogimport/Auswahl sind inzwischen angeschlossen; siehe [MCP-PRODUCT.md](MCP-PRODUCT.md). Native Schlüsselauflösung und
verschlüsselte Credentials ergänzt [MCP-SECRETS.md](MCP-SECRETS.md); vollständige
Einrichtung, Wiederherstellung und Server-KMS bleiben offen.

## Bestehende Sitzung erhalten

`Runtime::dispatch_persistent_mcp_tool` verlangt dieselbe konkrete Werkzeugaktion,
Lease und Freigabe wie das vorhandene Journal. Eine verweigerte Aktion erzeugt
auch keine neue HTTP-Verbindung. Bereits zugelassene Aufrufe liefern ausschließlich
ihren gespeicherten Versuch zurück.

Vor der ersten Initialisierung reserviert der Dienst einen Verbindungsdatensatz.
Nur die erste erfolgreiche Reservierung erhält die Gelegenheit zum Verbinden.
Nach `initialize`, `notifications/initialized` und geprüftem Katalog speichert er
den verschlüsselten Sitzungsstand. Spätere Aktionen rekonstruieren den Transport
mit derselben Sitzungskennung, derselben lokalen Verbindungsidentität und dem
aktuellen Verbrauchszähler. Sie senden weder eine neue Initialisierung noch eine
erneute Katalogentdeckung.

Das entspricht der Trennung zwischen einer MCP-Sitzung und ihren einzelnen
HTTP-Anfragen. Die Sitzungskennung gehört in die nachfolgenden Protokoll-Header;
ein Verbindungsabbruch beweist keinen Abbruch der entfernten Aktion.
[MCP Streamable HTTP und Session Management](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

## Atomarer Lebenszyklus

Migration 13 ergänzt die private Tabelle `mcp_connections`. Ein Lauf besitzt
höchstens eine Verbindung pro ausgewähltem Server. Ihre Zustände sind:

| Zustand | Bedeutung |
| --- | --- |
| `connecting` | Initialisierung reserviert; kein bestätigter Sitzungsstand |
| `ready` | Bestätigter verschlüsselter Checkpoint für die nächste zugelassene Aktion |
| `busy` | Genau ein Werkzeugaufruf ist dauerhaft zugelassen; Wiederherstellung gesperrt |
| `unavailable` | Kontext kann nicht sicher weiterverwendet werden; Prüfung erforderlich |

Die Werkzeugzulassung setzt die Verbindung gemeinsam mit Aufrufjournal,
Unsicherheitsmarker und Ereignis auf `busy`. Sie prüft zusätzlich die lokale
Verbindungsidentität und den Verbrauchszähler. Ein älterer authentischer
Checkpoint kann deshalb keine neue Sendeberechtigung erlangen.

Ein Erfolg schreibt Werkzeugergebnis, Nachricht, Schrittzähler, Ereignis und
neuen Sitzungs-Checkpoint in derselben Transaktion. Ein Ergebnisabschluss ohne
passenden neuen Checkpoint darf eine verwaltete Sitzung nicht freigeben. Bei
einer vom Server gemeldeten Katalogänderung bleibt das bestätigte Werkzeugergebnis
erhalten; die Verbindung wird jedoch für weitere Aktionen gesperrt.

Nach unklarem Ausgang wird die Verbindung `unavailable`. Der exakte menschliche
Ergebnisabgleich kann den Werkzeugschritt abschließen, aber nicht den alten
Sitzungsstand wieder freigeben. Nach verlorenem Initialisierungsabschluss bleibt
der Datensatz `connecting`. Weder dieser Fall noch ein HTTP-404 führt zu einer
automatischen Ersatzsitzung oder einer Werkzeugwiederholung. Eine erneute
Initialisierung benötigt eine separate ausdrückliche Auswahl; eine entsprechende
Abgleich-/Neuverbindungsoberfläche ist noch zu bauen.

## Schutz des Checkpoints

`SessionCipher` erhält einen 256-Bit-Schlüssel und eine Schlüsselkennung vom
vertrauenswürdigen nativen beziehungsweise serverseitigen Besitzer. Sie liest keine
Umgebungsvariablen, erzeugt keinen Ersatzschlüssel und speichert keinen Schlüssel
neben der Datenbank. Ein fehlender oder falscher Schlüssel blockiert die Fortsetzung.

Das interne Format verwendet `ring 0.17.14`: HKDF-SHA-256 leitet für jeden
Checkpoint aus einem frischen zufälligen 256-Bit-Salt einen eigenen Schlüssel für
ChaCha20-Poly1305 ab. Jeder abgeleitete Schlüssel wird einmal mit einem festen
Nonce zum Verschlüsseln verwendet. Dadurch hängt die Nonce-Trennung nicht von
einem nach Neustart zurückgesetzten Zähler ab. Format- und Schlüsselkennung sowie
der vollständige Gültigkeitsbereich sind authentifizierte Zusatzdaten. Die
Bibliothek verlangt eindeutige Nonces pro Verschlüsselungsschlüssel.
[ring AEAD-Vertrag](https://docs.rs/ring/0.17.14/ring/aead/struct.LessSafeKey.html)

Gebunden sind Projekt, Lauf, Server, Serverrevision, Verbindungsdatensatz,
Checkpoint-Revision, Konfigurationshash und Kataloghash. Der verschlüsselte Inhalt
enthält Protokollversion, Endpoint, lokale Verbindungsidentität, Verbrauchszähler,
Kataloghash und optional den geheimen `MCP-Session-Id`-Header. Endpoint und Katalog
werden beim Öffnen erneut mit der eingefrorenen Auswahl verglichen.

Bearer-Credentials sind nicht Bestandteil dieses Checkpoints. Sie werden getrennt
vom vertrauenswürdigen Besitzer geliefert. Der zusätzliche native Dienst löst sie
aus revisionierten verschlüsselten Datensätzen auf; dieser Sitzungs-Checkpoint
selbst enthält keine Bearer-Credentials. Schlüssel,
Session und Klartext-Checkpoint haben keine Debug- oder öffentliche
Serialisierungsschnittstelle. Temporäre Klartextpuffer und der aufbewahrte
Masterkey verwenden `Zeroizing`; dies ist keine Garantie vollständiger Löschung
aller Kopien in Bibliotheken, Betriebssystem oder Prozessspeicher.

Öffentliche Run-Details und Auftragsexporte enthalten weder verschlüsselte
Checkpoint-Blobs noch Protokoll-Sitzungsheader. Die vorhandenen lokalen
Verbindungskennungen im Werkzeugjournal sind davon verschiedene Zufallswerte.
Sitzungsereignisse enthalten ausschließlich Kennungen für die Zuordnung.
Die öffentlichen Vertragsdefinitionen, Ereignisschema 7 und Exportformat 9 bleiben
unverändert. Eine vollständige Datenbanksicherung enthält die verschlüsselten
Blobs; die dafür benötigten Schlüssel müssen separat gesichert werden.

## Nachweise und Grenzen

Zusätzlich zu den bisherigen MCP-Prüfungen gibt es zehn neue Prüfungen pro
Plattform: drei für Verschlüsselung und Bindungen, einen gemeinsamen
Speichervertrag sowie sechs Runtime-Prüfungen. Der Speichervertrag wird separat
gegen PostgreSQL ausgeführt. Die HTTP-Fixture vergibt echte synthetische
Sitzungsheader und verweigert unbekannte Kennungen.

Die Runtime-Prüfung beendet echte Worker-Kindprozesse an drei Stellen:
nach bestätigtem Ergebnis, nach dauerhaftem Remote-Append vor dessen Antwort und
während einer bereits begonnenen Initialisierung. Nur der erste Fall setzt dieselbe
Sitzung fort. Die beiden unbestätigten Fälle erzeugen keine Ersatzinitialisierung.
Weitere Fälle prüfen Freigaben, falsche Schlüssel, geänderte Ciphertexte und Zähler,
abgelaufene Remote-Sitzungen, Transaktionsfehler sowie fehlende Geheimnisse in
Auftragsexport und Testdatenbank.

Die Verschlüsselung authentifiziert den Checkpoint; sie schützt nicht gegen den
Rollback der gesamten Datenbank auf einen früheren authentischen Stand, einen
kompromittierten Schlüsselbesitzer oder beliebige entfernte Seiteneffekte.
Masterkey-Umschlüsselung/-wiederherstellung, authentifizierte Credential-Verwaltungsoberflächen,
Sitzungsbeendigung über DELETE, GET-Stream-Resume, ausdrückliche Neuverbindung,
UI sowie echte MCP-Server- und Enterprise-Abnahmen bleiben offen. Die kontrollierte
Append-Fixture beweist keine Beendigung beliebiger entfernter Programme.

```powershell
cargo test --locked -p cowork-mcp --lib
cargo test --locked -p cowork-storage --test mcp_connections
cargo test --locked -p cowork-runtime --test mcp_sessions
python tools/qualification/mcp.py --workspace
```

Aktuelle Gesamtprüfungen und Quellenmanifeste: [VERIFY.md](VERIFY.md).
