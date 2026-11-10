# M9: inaktive historische Quarantäne und lesende Vorschau

Aktueller Produktstand, 12. Oktober 2026: Zusätzlich zum hier beschriebenen
Offlineadapter ist der V2-Produktpfad in `historical_imports/product.rs`
angebunden. Native Desktop- und Organisationszugänge nehmen gebundene Archive
als verschlüsselte Quarantäne auf, erstellen eine aktuelle Zuordnung und
aktivieren ausdrücklich bestätigte Inhalte unter frischen V2-Rechten und
Speicherquoten. Native- und S3-Dateivorgänge besitzen eigene Originalquittungen.
Vollständiger Tabellenbrowser, paginierte Inhaltskataloge, kleinere editierbare
Inhaltsbereiche und spätere Aktivierungsbatches sind bis 0066 integriert. Das
exakte Schema66-Zielprofil und der eingefrorene
gemeinsame Windows-Build (private development evidence; omitted from public sources)
bestehen in ihrem Quellenumfang. Auch das eigene Schema67-Zielprofil ist
inzwischen exakt erzeugt/registriert und besitzt einen separaten gebundenen
Daemon-/Profilbeleg (private development evidence; omitted from public sources).
Für kalte Schema67-Kopien wird `current_quarantine_preview67.py` verwendet;
die älteren Producer und Profile bleiben unverändert. Reale Daten-/Datei-/Crash-/Linux-/
Import-/Installationsabnahme und aktuelle Gesamtpakete bleiben offen.
Die Feature-Matrix (development artifact omitted from public sources) führt diese verbleibenden Grenzen.

Die folgenden Abschnitte beschreiben den getrennten Offlineadapter und seine
historischen Nachweise. Er erhält seine ursprüngliche lesende Rolle.

`tools/migration/v1_quarantine.py` ergänzt die ersten beiden Schritte aus
`MIGRATION-V1.md` um einen eigenständigen Offline-Adapter für Schritt 3. Der
Adapter schreibt ausschließlich eine neu angelegte, getrennte Quarantäne oder
eine neu angelegte Vorschau. Er verändert keine V1- oder V2-Datenbank. Das ist
für sich kein V2-Produktimport und keine Aktivierung nach Schritt 4.

## Vollständiger historischer Bestand

Der Aufrufer wählt ein vollständiges `localai-cowork-v1-sqlite-history/1`-Paket
und pinnt den SHA-256 der exakten `export.json`-Datei. Ein inventarisiertes
Snapshot allein, ein Teilpaket oder eine Freigabe für einen anderen Zweck
genügt nicht. Das Paket muss exakt die bekannten Tabellen-Dateien enthalten.
Unbekannte Dateien, Schemas, Felder, Pfade, doppelte JSON-Schlüssel, falsche
Zeilenzahlen und geänderte Bytes werden abgewiesen.

Alle originalen kanonischen Zeilen werden unverändert als BLOBs in einer
versionierten SQLite-Datenbank mit `STRICT`-Tabellen gespeichert. NULL,
64-Bit-Integer, endliche REAL-Bits, Textbytes und BLOBs behalten ihre expliziten
Typen. Zusätzlich rekonstruiert der Adapter die komplette Quelldatenbank aus
den fest eingebetteten bekannten V1-Schemadefinitionen. Er liest die gesamten
rekonstruierten Typen und die Originalreihenfolge erneut und vergleicht sie
mit jedem Tabellenhash. SQL aus einem eingehenden Manifest wird niemals
ausgeführt. `integrity_check` und `foreign_key_check` müssen erfolgreich sein.

Die Quarantäne enthält ausschließlich `quarantine_meta`, `source_tables`,
`history_rows`, `history_projects`, `history_actors`, `history_entities` und
`history_relations`. Sie besitzt keine operative V2-Tabelle und wird vom V2
Store nicht als bekannte Datenbank akzeptiert. Ihr Status ist zwingend
`inactive`. Ein aktiver oder unklarer V1-Run, seine Approvals, Checkpoints,
Schedules, Reservierungen und Synchronisationsdaten bleiben historische
Bytes. Sie sind keine ausführbare Spec, kein Resume-Token und keine
Budget-, ACL- oder Replay-Autorität.

Der vollständige historische Export kann vertrauliche alte Inhalte oder
verschlüsselte Bindings enthalten. Die Quarantäne bewahrt diese originalen
historischen Bytes; die Vorschau gibt solche Inhalte nicht aus. Der Aufrufer
muss den ausdrücklich gewählten Export-/Quarantänepfad entsprechend schützen.
Die Read-only-Attribute sind kein Verschlüsselungs- oder Eigentümerschutz.

## IDs und Zuordnungen

Projekt-/Thread-/Run-/Artifact-Zuordnungen werden aus den ursprünglichen Zeilen
abgeleitet. Ein Desktop-`agent_personalities`-Eintrag ist eine Agentenpersona
und keine menschliche Identity. Beim Daemon stammen Projekt und ursprünglicher
menschlicher Creator aus `record_json.spec.project_id` beziehungsweise
`record_json.spec.creator_user_id`; widersprüchliche eingebettete Projektpins
werden abgewiesen. Der alte Helper zum Umschreiben eines Creators läuft nicht.

NULL- oder doppelte historische IDs, fehlende Referenzen, mehrdeutige
Artifact-/Run-Bindungen und Parent-Runs über unterschiedliche Projektgrenzen
werden abgewiesen. Quellen ohne bekannte Projektbindung werden als historische
Daten erhalten; operative Content-Mappings benötigen eine ausdrückliche,
vollständige ursprüngliche Projektzuordnung.

Die Mappingdatei hat `format: localai-cowork-historical-mapping/1`,
`purpose: historical_preview`, den originalen `export_sha256`, den exakten
`target_snapshot_sha256` und eine Liste `entries`. Jeder Eintrag enthält genau:

```json
{
  "source_kind": "project",
  "source_id": "<exakte historische Projekt-ID>",
  "target_kind": "project",
  "target_id": "<kanonische, nichtleere V2-UUID>",
  "namespace": "personal",
  "project_id": "<dieselbe V2-Projekt-UUID>",
  "organization_id": null,
  "creator_identity_id": null
}
```

Bekannte `source_kind`-Werte sind `project`, `thread`, `engine_run`, `crew_run`,
`daemon_run`, `artifact_version`, `engine_artifact`, `agent_persona` und
`historical_actor`. Mappings sind eindeutige historische Zuordnungen. Sie
beinhalten keine Credentials und können nichts aktivieren. Ein Eintrag für
eine nicht vorhandene ursprüngliche ID, mehrere Ziele für dieselbe Quelle
oder dieselbe Ziel-UUID für verschiedene Quellen werden abgewiesen.

Personal-Projekte bleiben von `tenant_projects` getrennt. Tenant-Mappings
müssen vorhandene Organisations-/Creator-IDs pinnen. Ein vorhandenes
Tenant-Projekt muss zu `identity_resources` mit `kind='project'`, derselben
Organisation und demselben ursprünglichen Creator passen. Personal-Zeilen
werden nicht automatisch einem Tenant zugeordnet. Ein Content-Mapping muss
die Pins seines ursprünglich zugeordneten Projekts übernehmen; beim Daemon
muss sein Tenant-Creator auch zur ausdrücklich gemappten alten Person passen.
Agentenpersonas können nicht auf `identity_subjects` umgedeutet werden.

## Bekannter, ausdrücklich kalter V2-Snapshot

Preview verlangt eine ausdrücklich gewählte selbständige V2-SQLite-Datei
ohne `-wal`, `-shm` oder `-journal` und deren exakten SHA-256. Das CLI sucht
keine App-Datenverzeichnisse und liest keine Live-Datenbanken. Originaldateien
werden nur über native OS-Handles gelesen; SQLite öffnet ausschließlich neue
private Kopien mit `mode=ro&immutable=1`, `query_only=ON` und
`trusted_schema=OFF`. Die Kopien werden nach der Prüfung entfernt.

`v2_snapshot_profile.py` enthält ein festes, geprüftes vollständiges
Strukturprofil: zentrale Storage-Migrationen 1–20 mit ihren originalen
SQL-Bytechecksums, die zusätzliche `messages_position`-Indexdefinition sowie
Identity 1, Tenant-Projekte 1 und die Accounting-/Personal-Money-Tabellen. 25
vollständige Quelldateihashes binden die Definitionen. Der Adapter liest
keinen fremden Code ein und extrahiert kein SQL zur Laufzeit. Geänderte
Quellbindings, Migrationchecksums, Guardmarker, zusätzliche/fehlende Objekte,
Integritäts- oder FK-Fehler benötigen einen neuen geprüften Adapter und führen
zum Abbruch.

Die Vorschau klassifiziert vorhandene Zielreferenzen und vorgeschlagene neue
historische IDs. Sie zeigt Original-/Ziel-IDs, Projektpins, relevante
Revisionen und begrenzte Ziel-Fakten. Sie exportiert keine V2-Passwörter,
Session-Tokens, JWTs, Credential-Payloads, ACLs oder Reservations. Die Prüfung
der ID-Kollisionen liest ausschließlich opaque IDs; sie authentifiziert keine
Session. Auch eine vorhandene aktive Subject-Zeile ergibt keine frische
Principal-Autorität. Diese muss Schritt 4 im produktiven V2-System prüfen.

Die Vorschau rekonstruiert alle ursprünglichen Zeilen und sämtliche abgeleiteten
Projekt-/Actor-/Content-Projektionen erneut. Ein nachträglich manipuliertes
und neu versiegeltes Quarantäne-Manifest macht geänderte Originalzeilen oder
Zuordnungen nicht vertrauenswürdig. Fehlende Mappings werden sichtbar als
`unmapped` ausgewiesen; `complete_mapping` ist nur eine Vollständigkeitsangabe
für die historischen Entities und kein Import-/Aktivierungsrecht.

## Transaktion, Freeze und Veröffentlichung

Unter Windows bleiben vollständige Eingabedateimengen und deren
Verzeichnis-Ahnen durch native Handles gegen Schreiben, Löschen und
Umbenennung geschützt. Dateiidentitäten, Größen und SHA-256 werden vor und
nach der Operation verglichen. Bereits vorhandene inkompatible Writer führen
zum Abbruch. Netzwerkpfade, Symlinks, Reparse Points und Hardlinks sind nicht
zugelassen. Unter Linux/macOS fehlt ein qualifizierter harter Source-Fence;
das CLI bricht dort ausdrücklich ab.

Alle History- und Projection-Zeilen werden in einer tatsächlichen SQLite-
Transaktion angelegt. Eine Ablehnung rollt die private Transaktion zurück.
Die vollständige Stage erhält erst nach Integrität, FK-Prüfung und finaler
Provenienzprüfung ein exklusives Windows-Rename auf einen neuen Zielpfad.
Bestehende Ausgaben werden nicht überschrieben. SQLite-Cursor und native
Handles schließen auch auf Fehlerpfaden. Nur die eigene, anhand ihrer
Dateisystemidentität geprüfte private Stage wird aufgeräumt.

Grenzen: 4 GiB Original-JSONL, 12 GiB physische Quarantäne einschließlich SQLite-
Overhead, 512 MiB kalter V2-Snapshot, höchstens 1.000.000 Zeilen, 16 MiB je originalem
SQLite-Wert, 256 MiB je kanonischer Zeile, 2 MiB je JSON-Manifest/Mapping und
120 Sekunden kooperatives Operationsbudget. Validierung und Rekonstruktion
benötigen zusätzliche private Plattenkapazität. Das Budget ist kein harter
Prozess-Timeout für blockierendes OS-I/O. Die qualifizierte Testsuite läuft
unter einem eigenen Windows Job und bestätigt tatsächliche Prozess-Exits.

## CLI

Für ein kaltes aktuelles Schema67-Ziel verwendet der Produktpfad die gesonderte
versionierte Vorschau. Sämtliche Dateipfade sind absolute lokale Pfade; das
Ausgabeverzeichnis muss neu sein. Alte Schema20/64/66-Aufrufe behalten ihre
ursprünglichen Producer und Pins.

```powershell
python -B tools/migration/current_quarantine_preview67.py `
  --target-profile v2-storage-67-complete-store-connect/1 `
  --quarantine C:\MigrationQuarantine\history-001\quarantine.json `
  --quarantine-sha256 <exakter Quarantäne-Manifest-Dateihash> `
  --target-snapshot C:\MigrationInput\v2-cold.sqlite3 `
  --target-snapshot-sha256 <exakter kalter Snapshot-Dateihash> `
  --mapping C:\MigrationMapping\mapping-001.json `
  --mapping-sha256 <exakter Mapping-Dateihash> `
  --output C:\MigrationPreview\schema67-preview-001
```

Die folgenden Befehle und ursprünglichen Grenzen beschreiben den separat
qualifizierten historischen Offlineadapter.

```powershell
python -B tools/migration/v1_quarantine.py quarantine `
  --export C:\MigrationExport\export-001\export.json `
  --export-sha256 <exakter Export-Dateihash> `
  --output C:\MigrationQuarantine\history-001

python -B tools/migration/v1_quarantine.py preview `
  --quarantine C:\MigrationQuarantine\history-001\quarantine.json `
  --quarantine-sha256 <exakter Quarantäne-Manifest-Dateihash> `
  --target-snapshot C:\MigrationInput\v2-cold.sqlite3 `
  --target-snapshot-sha256 <exakter kalter Snapshot-Dateihash> `
  --mapping C:\MigrationMapping\mapping-001.json `
  --mapping-sha256 <exakter Mapping-Dateihash> `
  --output C:\MigrationPreview\preview-001
```

Exit 0 bedeutet eine vollständige geprüfte inaktive Quarantäne beziehungsweise
lesende Vorschau; Exit 2 bedeutet Abweisung. Eine saubere Vorschau schreibt
keine V2-Produktdaten. Produktimport, Android-AES-GCM-Adapter, frische
Principal-/Content-Autorisierung, Konfliktauflösung und ausdrückliche
Aktivierung nach Schritt 4 sowie der vollständige M9-Abschluss bleiben offen.

Die Tests erzeugen ausschließlich neue private SQLite-Fixtures:

```powershell
python -B tools/migration/test_v1_quarantine.py `
  --evidence-dir C:\PrivateProofs\m9-quarantine-new-guid
```

Der Evidence-Report bindet den tatsächlichen Python-Quellenstand. Erfasst
werden Originaldateihashes, der Schutz gegen SQLite-Öffnen der Originale,
echte Native-CLI-Exits, Cursor-/Handlefreigabe, UNKNOWN-History, Manipulationen,
Rollback, ID-/Projektkonflikte und die Trennung der Tenant-/Personal-Schemas.
Die Fixtures belegen diese Offline-Grenze; sie ersetzen keinen produktiven
V2-Import, keine Live-Authentifizierung und keine Android-/Lastqualifikation.
