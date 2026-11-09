# V1-Datenmigration: lesendes Inventar und historischer Export

Aktueller Produktstand, 12. Oktober 2026: Der getrennte V2-Produktimport und
die ausdrückliche Inhaltsaktivierung sind inzwischen in Storage, Daemon,
Desktop-IPC und DE/EN-Oberfläche eingebunden. Der Originalbestand bleibt
verschlüsselte historische Quarantäne. Aktuelle Rechte, Quoten und separate
Dateiquittungen entscheiden über jede Übernahme; alte Actor-/ACL-/Run-Daten
werden nicht als heutige Autorität übernommen. Vollständiger Historienbrowser,
große einzeln zuordenbare Inhaltsbereiche und weitere Aktivierungsbatches
desselben Imports sind bis 0066 integriert. Schema66-Zielprofil und
gemeinsamer eingefrorener Windows-Build (private development evidence; omitted from public sources)
bestehen. Auch das separate Schema67-Zielprofil ist inzwischen erzeugt und
exakt registriert; sein eigener
Daemon-/Profilbeleg (private development evidence; omitted from public sources)
rekonstruiert 471 DDL-Anweisungen. Der neue Vorschau-Producer
`current_quarantine_preview67.py` erhält sämtliche alten Producerpins;
siehe den aktuellen CLI-Aufruf in [QUARANTINE.md](QUARANTINE.md).
Reale Versions-/Daten-/Datei-/Crash-/Linux-/
Installationsabnahme und aktuelle Gesamtpakete bleiben offen;
siehe Feature-Matrix (development artifact omitted from public sources).

Die nachfolgenden CLI- und Siegelbeschreibungen bleiben die historischen
Nachweise des getrennten lesenden Export-/Verifierpfads. Sie qualifizieren
keinen späteren Produktstand automatisch.

`tools/migration/v1_sqlite_export.py` implementiert die ersten beiden M9-Schritte als eigenständiges Python-CLI mit Standardbibliothek auf Windows und Linux. Es erzeugt keine V2-Datenbankeinträge, Identitäten, Mitgliedschaften, Credentials, Berechtigungen, Run-Reservierungen oder Aktivierungen. Bestehende aktive/unklare Runs, Approvals, Eingaben, Schedules, Prozess- und Synchronisationszustände bleiben unveränderte historische Daten. Kein ausgegebenes Paket ist ausführbare Run- oder Synchronisationsautorität.

## Vier Schritte und historische CLI-Grenze

Der reine Rust-Verifier in `cowork-storage` ist auf Windows und Ubuntu mit
30 beziehungsweise 29 Tests sowie Format und Clippy geprüft. Das
unveränderte Produktsiegel (private development evidence; omitted from public sources)
bindet 1873 Eingaben, 36 eingebettete Referenzen und 1630 synthetische
Fixturedateien an ihren damaligen Quellenstand. Nachfolgende Daemon-/Broker-
Korrekturen und aktuelle native Prozessprüfungen sind separat belegt; daraus
wird keine erneute vollständige Prüfung dieses historischen Siegelumfangs
abgeleitet. Es implementiert weder einen HTTP-/UI-Import noch die
Live-Aktivierung, Migration 22 oder den Android-Adapter. Kalte Previewkopien
geben keine Schreibrechte auf die laufende V2-Datenbank.

| Schritt | Stand | Ergebnis |
| --- | --- | --- |
| 1. Versioniertes Source-Inventar | Implementiert für bekannte V1-Desktop-/Daemon-SQLite-Strukturen auf Windows und leasefähigen lokalen Linux-Dateisystemen | Eingefrorene Byteprovenienz, konsistenter privater Snapshot, Schemaobjekte, Integrität, FK-Prüfung, Tabellen-/Inhaltsbelege |
| 2. Freigegebener lesender Export | Implementiert als lokale, digestgebundene historische Freigabe | Kanonische, gestreamte Tabellen-JSONL und versiegeltes Exportmanifest |
| 3. Quarantäne-Import und Preview/Diff in V2 | Separater Offlineadapter und reiner Storage-Verifier integriert; produktiver Importpfad offen | Vollständige inaktive historische Quarantäne und digestgebundene Zuordnungen/Preview gegen kalte V2-Snapshotkopien; keine aktuelle Inhaltsautorität |
| 4. Explizite Aktivierung mit frischer V2-Autorität | Offen | Keine Wiederverwendung alter Tokens, Berechtigungen, Approvals, Reservierungen, Schedules oder Resume-/Replay-Autorität |

Android ist ein separater offener Adapter: `app/src/mobile/mobileOfflineStore.ts` speichert `open-cowork-mobile-cache-v1` mit `schemaVersion: 1` als AES-GCM-verschlüsselten LocalStorage-Zustand (12-Byte-IV, Android-Keystore-Schlüssel und AAD). Das ist keine SQLite-Datenbank. Dieses CLI entschlüsselt den Cache nicht und importiert keine Outbox. V1 `/api/v1/version` prüft Schema 2 mit Minimum 1; Device-Hello/Heartbeat haben eine eigene Kompatibilitätsprüfung. Ein V1↔V2-Android-Protokolladapter samt Aushandlung, Preview, Offline-/Konfliktmatrix und Aktivierung ist noch nicht implementiert.

## Versions- und Quellenbindung

Die eingebetteten, festen Referenzdefinitionen in `schema_profiles.py` stammen aus dem unveränderten Datenbankcode des separat generierten M0-Kandidaten. Die vollständigen Quellen sind mit folgenden SHA-256-Werten gebunden:

| V1-Quelle | SHA-256 | Gelesenes Format |
| --- | --- | --- |
| `app/src-tauri/src/db.rs` | `04ce864726e5bb6a6c9aab4ae2fe4583055f68998ce1ccffcf6ce4a727645e20` | `Database::migrate`, Desktop `schema_version` 0–28 |
| `agents/cowork-local-daemon/src/main.rs` | `ae6c476d0d1a168b287d23973b64f1ec76ba337dfde888c2c908e1575f0e0df2` | `initialize_database`, 13 Anwendungstabellen ohne numerischen DB-Schemaversion-Marker |
| `agents/cowork-local-daemon/src/sync_ipc.rs` | `c8c9b28e1104507c0df9ec925452b12519ef6134fead8ecb0d9218dc9dc3de31` | `ensure_schema`, zwei zusätzliche Peer-/Konflikttabellen |

Diese SQL-Definitionen werden ausschließlich zur Konstruktion privater In-Memory-Referenzstrukturen ausgeführt. Die Quelldatenbank wird weder migriert noch mit SQLite geöffnet. Eine neue/abweichende Struktur benötigt einen neuen geprüften Adapter; das CLI übernimmt keine unbekannte Struktur automatisch.

Desktop 0 akzeptiert eine echte leere SQLite-Datei oder die bekannte `schema_version`-Tabelle mit genau einer Integer-Zeile 0. Für 1–28 muss genau eine Integer-Zeile zum bekannten gesamten Schema passen. Leere/mehrfache/falsch typisierte Marker, negative Versionen und Versionen über 28 werden abgewiesen. Die Struktur beinhaltet bis zu 49 Tabellen einschließlich Versionsmarker. Für 27/28 sind die beiden ausdrücklich bekannten Varianten mit/ohne `work_tasks.backend_selection_json` gebunden; dieser V1-Kompatibilitäts-Backpatch besitzt keine zusätzliche Versionsnummer.

Beim Daemon werden die exakten Profile `daemon-base-13` und `daemon-sync-15` erkannt. Interne SQLite-Objekte, einschließlich `sqlite_sequence`, werden zusätzlich erfasst. `PRAGMA user_version` und `application_id` müssen den bekannten Wert 0 behalten; sie werden nicht zu einer erfundenen Daemon-Version umgedeutet. Der V1-Helper `migrate_legacy_creator_user_id` wird nicht aufgerufen.

Schema-Fingerprints binden Tabellen, Spalten, Defaults, PKs, FKs, Indexmetadaten und sämtliche `sqlite_schema`-Objekte mit SQL. Außerhalb von SQL-Literalen wird unwichtiger Whitespace/Keyword-Case normalisiert. Inventare enthalten auch vollständigen SQL-Text für Indizes, Views und Trigger. Die bekannten Profile besitzen keine Anwendungstrigger/-views; zusätzliche Objekte werden als unbekannte Struktur abgewiesen.

## Source-Fence und Snapshot

Unter Windows werden ausschließlich absolute lokale Pfade auf festen/RAM-Laufwerken verwendet. UNC-/Netzlaufwerke, Traversal, Symlinks, Reparse Points, Datei-Hardlinks, fehlende Komponenten und Pfade über 240 Zeichen werden abgewiesen. Das Output-Verzeichnis muss neu sein, einen existierenden Elternpfad haben und außerhalb aller Eingabebäume liegen. Dateien werden exklusiv neu erstellt. Quelldateien und alle betroffenen Verzeichnis-Ahnen bleiben durch native Handles gegen Schreiben/Löschen beziehungsweise Namespace-Umbenennung geschützt. Ein bereits vorhandener inkompatibler Writer führt zum Abbruch. Der Verzeichnis-Fence verwendet `FILE_LIST_DIRECTORY`; `FILE_READ_ATTRIBUTES` allein wurde durch eine echte Rename-Gegenprobe als unzureichend erkannt.

Die Quelle wird nur über OS-Dateihandles gelesen. Ein gemeinsam gehaltener Freeze umfasst die DB und vorhandene `-wal`, `-shm` und `-journal`-Dateien. Dateiidentität, Pfadidentität, Größen, Seitendateimenge und SHA-256 werden davor/danach geprüft. DB/WAL/SHM werden bytegetreu in eine private Stage kopiert. Nichtleere Rollback-Journale sowie unbekannte/trunkierte/beschädigte WAL-Header/-Frames, falsche Salts/Checksummen und uncommitted WAL-Tails werden abgewiesen. Ein gültiger committed WAL wird ausdrücklich berücksichtigt.

Nur die private Kopie wird mit SQLite `mode=ro` geöffnet. `Connection.backup` erzeugt daraus einen selbständigen privaten Snapshot. Dieser wird anschließend mit `mode=ro&immutable=1`, `query_only=ON` und `trusted_schema=OFF` gelesen. Ein direktes `immutable=1` auf einer Quelle mit WAL wäre kein ausreichender Snapshot und wird nicht verwendet. `integrity_check(1)` muss `ok` liefern; jede FK-Verletzung wird auch bei ansonsten erfolgreicher Integritätsprüfung abgewiesen. Das Snapshot-Bytehash sowie Inhaltshashes/Zeilenzahlen binden den logischen Export an die ursprüngliche physische DB-/WAL-/SHM-Provenienz.

Der Linux-Adapter `linux_source_fence.py` hält echte lesende
[`F_SETLEASE`-Leases](https://raw.githubusercontent.com/mkerrisk/man-pages/master/man2/fcntl.2)
auf allen Eingaben. Ein vorhandener schreibender Descriptor oder ein schreibbares
Mapping verhindert die Zulassung. Ein neuer Write-Open/Truncate erzeugt eine
bleibende Abweisung; der Export gibt nur seine eigenen Leases frei und beendet
keinen Writer. Auch nach Ablauf einer Kernel-Breakfrist wird eine verlorene Lease
nicht wieder erworben. Quellenchecks laufen während Hash/Copy, in den
SQLite-Progresscallbacks der privaten Kopien und vor der Veröffentlichung.

Gehaltene, ohne Symlinkfolge geöffnete Verzeichnisdeskriptoren binden jeden
Pfadvorfahren. Eine eigene
[`inotify`-Instanz](https://raw.githubusercontent.com/mkerrisk/man-pages/master/man7/inotify.7)
erkennt Änderungen an den genauen Eingaben, auch fehlenden SQLite-Sidecars und
zwischenzeitlich umbenannten/zurückbenannten Vorfahren. Queueverlust, Overflow,
Unmount, veränderte Identitäten, Hardlinks und fremde Dateieigentümer verweigern
den Vorgang. Linux-Verzeichnisse werden damit beobachtet und über Deskriptoren
angesprochen; es wird kein Windows-artiges Verbot jedes fremden Rename behauptet.

Die aktuelle Veröffentlichung verwendet exklusive native Verzeichnisanlage
und FD-relative Kopie, ohne einen austauschbaren Source-Namen umzubenennen.
Ein separater geflushter Marker im ursprünglichen Elternverzeichnis mit Namen
`.m9-linux-pending-<SHA256-des-Zielnamens>` entsteht vor dem Ziel. Alle
Inventar-/Export-/Quarantäne-Dateifences prüfen solche Marker für jeden Vorfahren;
auch ein ausgetauschter Zielname bleibt so für den Import gesperrt.
Genau die eigenen Create/Delete-Ereignisse werden beobachtet; zusätzliche
Namensänderungen, verlorene Ereignisse oder veränderte Directory-FDs verweigern
den Vorgang vor einer weiteren Publikationsaktion. Originale Stage-Dateien bleiben
beim Kopieren geleast, und die neuen Zielidentitäten/-bytes werden unter eigenen
Read-Leases gegen die tatsächlichen Kopierbelege geprüft. Nur das bewiesene eigene
Publikationsobjekt darf während dieses internen Schritts seinen Marker übergehen;
gewöhnliche Intake-APIs erhalten diese Ausnahme nicht.

Erst nach vollständiger Prüfung und Bereinigung des eigenen Originalstage wird
der genaue eigene Marker entfernt. Ein vorhandenes Ziel wird nie überschrieben;
ein fremder Stage-Ersatz wird weder verschoben noch übernommen. Fehler behalten
die unbestätigte Ausgabe und ihren unabhängigen Marker, ohne automatische
Bestätigung oder Löschung fremder Daten. Die Bereinigung eigener Stages läuft
FD-relativ nach Identitäts- und vollständiger Link-/Typ-/Eigentümerprüfung.

Der native Pfad ist für ein eigenes einzelthreadiges 64-Bit-Linux-CLI vorgesehen.
Es verwaltet vorübergehend eigene geblockte Lease-Signale über `signalfd`, prüft
bestehende Signalownership und stellt die alte Maske wieder her. Es ist kein
Signaladapter für einen eingebetteten mehrthreadigen Worker. Unterstützt sind
lokale Ext2/3/4-, XFS-, Btrfs- und tmpfs-Mounts, sofern tatsächliche Leases und
exklusive Verzeichnisanlage unterstützt wird und die Originalidentitäten in das
historische Belegformat passen. Netzwerk-, FUSE-/DrvFs-/9p-Quellen und andere
Dateisysteme werden ausdrücklich abgewiesen. Windows-Pfade unter `/mnt/c` sind
daher keine geeigneten Linux-Migrationsquellen. macOS bleibt außerhalb des
beauftragten Umfangs. Kein `flock`, Mountwechsel, Filesystem-Freeze, Sysctlwechsel
oder globaler Dienstneustart wird verwendet.

Ein enger echter Ubuntu/WSL-Probe auf eigenen Ext4-Dateien besteht zehn
Beobachtungsgruppen in 3,175 Sekunden: committed WAL → Inventar → digestgebundene
Freigabe → Export → vollständige inaktive Quarantäne; kein ursprünglicher SQLite-
Open; bestehender Writer und verbleibendes writable mmap abgewiesen; neuer nativer
Write-Open mit positiv beendetem Originalkind; transiente Sidecar- und
Vorfahrenänderungen erkannt; Hardlink/Symlink abgewiesen; tatsächliches No-replace;
unbestätigter Post-Rename-Output wird erhalten und als Input abgewiesen; nachher
unveränderte Sourcebytes, erfolgreiche Write-Reopens, restaurierte Signalmaske und
keine gehaltenen eigenen Handles. Das ist kein neuer vollständiger Versionsmatrix-,
physischer V1-Installations- oder Aktivierungsbeleg. Die älteren Windowsbelege
bleiben an ihre ursprünglichen Quellen gebunden.

Der anschließende historische native Previewbeleg (private development evidence; omitted from public sources)
ergänzt die tatsächliche lesende Stage-3-Vorschau: elf Beobachtungsgruppen in
4,051 Sekunden, sämtliche 25 ursprünglichen Profilreferenzen unter echten
Linux-Dateifences, unveränderte kalte Zieldatenbank, kein SQLite-Open der
ursprünglichen Ziel-/Quarantänedatenbank und keine Zielcredentialwerte im Ergebnis.
Zwei getrennte einzelne Windowsfälle bestehen (WAL-Export: 0,245 Sekunden;
Vorschau bestehender IDs ohne Tokens/Grants: 1,623 Sekunden). Der erste erweiterte
Linuxversuch wurde korrekt wegen einer fehlerhaft im Output-Ahnenbaum angelegten
Fixture abgewiesen; die Fixture wurde korrigiert, die Overlapprüfung blieb erhalten.
Der frühere Zehngruppenbeleg bleibt unverändert historisch.

Die Vorschau verwendet nun `tools/migration/reference_source`: 25 originale,
insgesamt 91.922 Byte große Profilquellen, aus dem unveränderten historischen
Quarantäne-Quellarchiv extrahiert und gegen alle festen `SOURCE_BINDINGS`
geprüft. Der aktuelle Core darf sich weiterentwickeln, ohne diese Originalpins
zu ersetzen. `prepare_profile_reference.py` prüft Archivpin, originale Member-
CRCs/Bytes und feste Quellenhashes und veröffentlicht nur eine neue private
Referenzkopie. Profildefinition, alte Zielschemaprüfung und Mappings werden
dadurch nicht erweitert. Die Releasevorbereitung bindet alle mitgelieferten
Migrationshelfer und Referenzdateien an ihre tatsächlichen Bytes.

Für ein eigenes ursprüngliches, unverändertes Profilquellverzeichnis akzeptiert
`v1_quarantine.py preview` zusätzlich `--profile-source-root <absolute-directory>`.
Auch dann müssen alle 25 festen Quellenhashes unter denselben nativen Fences
stimmen. Das ermöglicht unter Linux eine eigene operator-eigene Referenzkopie
auf einem unterstützten Dateisystem. Ohne den Parameter wird die mitgelieferte
Referenz verwendet; alte archivierte Bäume ohne Referenz behalten ihr bisheriges
Root-Verhalten. Weder Archivmetadaten noch JSON können unbekannte Quellpfade
oder neue Schema-/Ausführungsautorität einschleusen.

Eine unabhängige Quellprüfung fand danach das Source-Namenswechsel-Fenster des
ersten Rename-Publishers. Dieser Publisher wurde durch die oben beschriebene
exklusive FD-Kopie ersetzt; frühere grüne Belege bleiben historisch. Der
vorherige native Publikationsbeleg (private development evidence; omitted from public sources)
besteht zwölf Gruppen in 8,713 Sekunden, einschließlich tatsächlichem
Stage-Namensersatz: der fremde Ersatz bleibt am gleichen Ort und bytegleich,
kein fremdes Directory wird bewegt/adoptiert, und der unabhängige Marker bleibt
zur ausdrücklichen Klärung erhalten. Der vollständige WAL-/Vorschau-/Writer-/
Namespace-/Reopenpfad besteht auf denselben aktuellen Helferbytes.

Die zweite enge Quellprüfung ergänzte die ursprüngliche Operationdeadline auch
für die neue Kopie, Ziel-Vollhashprüfung und Baumbereinigung; dafür wird keine
neue Laufzeit vergeben. Der historische Export prüft unmittelbar vor der
Bestätigung zusätzlich die ursprüngliche Freigabe mit frischer UTC-Zeit. Der
aktuelle Schlussbeleg (private development evidence; omitted from public sources)
besteht dreizehn Beobachtungsgruppen in 9,957 Sekunden, einschließlich
Deadlineüberschreitung während der Publikationskopie mit erhaltenem Sperrmarker.
Der einzelne aktuelle Windows-Previewfall besteht erneut in 1,590 Sekunden.
Das ist keine neue vollständige Windows-/Linux-Versionsmatrix oder physische
M9-Installations-/Aktivierungsabnahme.

## Kanonischer Inhalt und Grenzen

Jede Tabelle wird in PK-/Rowid-Reihenfolge gestreamt; die physische Rowid wird zur eindeutigen Bindung auch bei nullable TEXT-PKs aufgenommen. Spaltennamen/-reihenfolge stehen im Manifest. Es gibt keine komplette Tabellen-/Datenbank-`readall`-Operation. Typen bleiben explizit:

- NULL: `{"type":"null"}`.
- INTEGER: exakte dezimale Zeichenkette des SQLite-64-Bit-Werts.
- REAL: nur endliche Werte; IEEE-754-Binary64-Big-Endian-Hex des von SQLite gelesenen logischen Werts. Der separate Quelldateihash bewahrt die Bindung an physische Seitenbytes.
- TEXT: unveränderte SQLite-UTF-8-Bytes als Base64, auch für ungültige UTF-8-Folgen.
- BLOB: unveränderte Bytes als Base64.

Der Formatbezeichner lautet `localai-cowork-v1-sqlite-history/1`. SHA-256 pro Tabellen-JSONL bindet jede Zeile und ihre Reihenfolge; das Content-Manifest bindet Spalten-/Dateizuordnung. `manifest_sha256` bindet die kanonischen Manifestfelder ohne dieses Feld selbst. Zusätzlich wird der SHA-256 der exakten Manifestdatei (einschließlich Abschlussnewline) ausgegeben und bei Freigabe gepinnt.

Feste Grenzen: insgesamt 512 MiB eingefrorene Eingabedateien, 1.000.000 Tabellenzeilen, 16 MiB SQLite-Zell-/Wertgrenze, 4 GiB gestreamte kanonische Daten, 2 MiB je gelesener Freigabe-/Inventardatei und 120 Sekunden kooperatives Operationsbudget. Chunk-/WAL-/Row-Schleifen und SQLite-Progress/Backup prüfen das Budget. Betriebssystem-I/O-Blocking ist dadurch kein harter Prozess-Timeout. Nichtendliche REALs, unbekannte JSON-Felder der Autorität und doppelte JSON-Schlüssel werden abgewiesen.

Die private Stage wird erst vollständig geprüft veröffentlicht. Windows-Rename überschreibt kein vorhandenes Ziel. Fertige Dateien erhalten das Read-only-Attribut. Das ist ein exklusiv neu erstelltes, contentversiegeltes Artefakt; ein Dateibesitzer kann Attribute/Bytes später ändern. Änderungen machen die gebundenen Hashes ungültig. Fehlgeschlagene Stages werden nur bei passender eigener Verzeichnisidentität entfernt; native Handles schließen auch bei Fehlern. Kein globaler Python-/Cargo-/npm-/PATH-/DB-Zustand wird verändert.

## Bedienung

Getestet mit nativem Windows, Python 3.14 und SQLite 3.50.4. Python 3.11+ ist wegen `Connection.setlimit` erforderlich. Es gibt keine Paketinstallation, Cargo-Workspace- oder Root-Abhängigkeitsänderung.

Beispiel für ein **explizit ausgewähltes, zuvor geschlossenes** V1-Desktop-DB-Verzeichnis und getrennte Ausgabepfade:

```powershell
python -B tools/migration/v1_sqlite_export.py inventory `
  --source C:\MigrationInput\desktop\open_cowork.db `
  --family desktop --output C:\MigrationOutput\inventory-001
```

Das CLI gibt `approved_inventory_sha256` zurück. Nach Prüfung des Inventars kann der Aufrufer eine eigene, neue lokale Autoritätsdatei außerhalb des Inventar-/Exportbaums bereitstellen:

```json
{
  "format": "localai-cowork-v1-sqlite-history/1",
  "state": "approved",
  "purpose": "historical_export",
  "inventory_sha256": "<exakter SHA-256 der geprüften inventory.json-Datei>",
  "expires_at": "<zukünftiger UTC-Zeitpunkt im ISO-8601-Format>"
}
```

Der Aufrufer pinnt auch den SHA-256 dieser exakten Datei. Eine `revoked`-, abgelaufene oder anderweitig zweckgebundene Autorität wird abgewiesen. Freigabedatei, Inventar und Snapshot werden gemeinsam eingefroren und bis nach Veröffentlichung gehalten. Das ist eine lokale Freigabe des historischen Exports zum Freeze-Zeitpunkt; keine Verbindung zu V2-Identity, kein laufender verteilter Revocation-Mechanismus und kein Aktivierungsrecht. Erneute Freigaben erfolgen über neue Dateien/digestgebundene Aufrufe. Ein bereits erzeugter historischer Export erlangt durch eine spätere Freigabeänderung keine Produktrechte.

```powershell
python -B tools/migration/v1_sqlite_export.py export `
  --inventory C:\MigrationOutput\inventory-001\inventory.json `
  --approved-inventory-sha256 <inventory-dateihash> `
  --authority C:\MigrationApproval\approval-001.json `
  --authority-sha256 <approval-dateihash> `
  --output C:\MigrationExport\export-001
```

Exit 0 bedeutet erfolgreichen geprüften Export; Exit 2 bedeutet Abweisung. Der
separate Offline-Schritt 3 ist in [QUARANTINE.md](QUARANTINE.md) beschrieben und
auf seinem eigenen eingefrorenen Stand qualifiziert. Produktiver Rust-Import,
M9-Gesamtabschluss, Android, Produkt-Autorisierung und Aktivierung bleiben offen.

## Fixture-Qualifikation

`test_v1_sqlite_export.py` legt ausschließlich frische private SQLite-Fixtures an. Es sucht keine App-Datenverzeichnisse und öffnet keine Live-Datenbanken:

```powershell
python -B tools/migration/test_v1_sqlite_export.py `
  --evidence-dir C:\PrivateProofs\m9-fixtures-new-guid
```

Die tatsächlichen Tests decken Desktop 0–28 sowie beide 27/28-Backpatch-Varianten, Daemon 13/15, committed WAL-only Rows, beschädigte Checksummen und einen von SQLite tatsächlich gespillten uncommitted WAL-Tail ab. Sie prüfen die unveränderten Quellhashes auch nach Abweisung, instrumentieren SQLite gegen das Öffnen der Originalquelle, lehnen echte FK-Orphans trotz `integrity_check=ok` ab, erkennen manipulierte/resealed Freigaben und Snapshotbytes, sowie revoked/expired/wrong-purpose Autorität. Kanonische Gegenproben umfassen 64-Bit-Integergrenzen, REAL-Bits, NULL, BLOB und ungültiges UTF-8. Echte native Kindprozesse belegen Schreibverweigerung und CLI-Exits mit positivem `wait`/Reap; Handles werden danach durch erfolgreiche Write-Reopens und Namespace-Renames freigegeben bestätigt. Der Report bindet die Python-Quellhashes und bleibt als getrenntes privates Evidence-Artefakt erhalten.


### Android-Cache und laufende Metadaten (Schema 72)

Die aktuelle persönliche Projektansicht bietet getrennte Bereiche für Projektfreigaben, befristeten lesenden Support, Metadatensynchronisierung und den mobilen Cacheimport. Der Cacheexport wird in der aktualisierten V1-Mobile-App unter den Einstellungen ausdrücklich erzeugt; ein bestehender Android-Keystore-Schlüssel wird dabei nur gelesen und niemals exportiert oder neu angelegt. Die neue Produzentenquelle ist kein Nachweis einer bereits installierten APK oder eines durchgeführten Exports auf einem Telefon.

Im V2-Cachebereich wählt der Benutzer die unveränderte Exportdatei (höchstens 24 MiB). Das Backend speichert sie verschlüsselt in Teilen von höchstens 1 MiB und prüft Größe und SHA256. Bei einer verlorenen Antwort wird zuerst die ursprüngliche Operation bzw. der aktuelle Import samt bereits angenommenen Teilen gelesen. Der Index erhält den vollständigen originalen Payload; Einträge und Downloads sind paginiert. Ein späterer Export enthält weder V1-Keystore-Schlüssel noch eine Behauptung, dass die alte Benutzer-ID die aktuelle Sitzung autorisiert.

Die ausdrückliche Aktivierung kopiert ausgewählte Nachrichten mit neuen IDs in ein aktuell beschreibbares persönliches oder geteiltes Gespräch. Die übrigen Originale bleiben Quarantänehistorie. Alte Outbox-Operationen, Runs, Freigaben, Zugangsdaten und Zeitpläne werden nicht ausgeführt. Eine neue Ausführung braucht die regulären aktuellen Projekt-, Modell-, Werkzeug- und Gerätefreigaben. Bereits kopierte Nachrichten werden nicht erneut unter derselben Quittung eingefügt. Bei einem unvollständigen Import kann die ausdrückliche Aufbewahrung die noch ungenutzte Speicherreservation schließen; sie erklärt fehlende Teile niemals zu empfangenen Originalbytes.

Der aktuelle Platformadmin kann die tatsächliche persönliche Speichergrenze mit einer erwarteten Revision im Verwaltungsbereich setzen (Standard 512 MiB, maximal 16 GiB; verbrauchter und reservierter Speicher kann nicht still zurückgesetzt werden). Dies ist eine Speicherquote, kein neues Token-, Modell- oder Geldbudget. Organisatorische Windows-Pools benötigen ein aktiviertes Profil, eine aktuelle Projektzuordnung und tatsächlich gekoppelte passende Geräte. Eine Geräte-Sync-Freigabe ist zusätzlich ausdrücklich zu erteilen und bleibt an die ursprüngliche aktuelle Sitzung und Projektberechtigung gebunden. Provider-, Run-, Schedule-, Crew- und Werkzeugmetadaten enthalten keine ausführbaren oder geerbten Credentialgrants.

### Android cache and ongoing metadata (schema 72)

The current personal project view separates project sharing, expiring read-only support, metadata synchronization and mobile cache intake. Explicitly export the unchanged cache through Settings in the updated V1 mobile producer, then select it in V2. The producer reads the existing Android key without exporting or creating it; its source receipt does not qualify an installed APK or physical phone export. Intake is limited to 24 MiB and authenticated chunks to 1 MiB. Recover an uncertain response by reading its original operation and received chunk pins before choosing or reopening the source file.

Explicit activation creates new inert messages in a currently writable personal or shared conversation. All remaining original payload, events, runs and outbox records stay historical; none grants a current account, credential, execution or resume right. A platform administrator can CAS-change the actual personal storage limit from the 512 MiB default up to 16 GiB without resetting existing usage or reservations. Managed Windows pools and device replication each require their own current explicit project admission, beyond pairing. Runtime, provider, APK and physical failure/recovery qualification and current installation packages remain separate release evidence.
