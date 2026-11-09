# Lokal vorbereitete Ausführungseingaben

Die neue Owner-API friert ausdrücklich ausgewählte lokale Ordner in unveränderliche
Inhalte ein. Sie erzeugt keinen Lauf, ruft kein Modell auf und überträgt keine
Dateiinhalte an einen Runner. Der bestehende `ExecutionPlan` Format 1 behält
`input_snapshot_bound=false`. Der neue Owner-Plan Format 2 bindet diese Eingaben
an einen ausdrücklich prozessfreien Dateibroker und eine eigene Arbeitskopie
pro Lauf. Nur dieser angebundene Pfad meldet `input_snapshot_bound=true`.

## Bestätigte Arbeitskopien

`PrepareOwnerExecutionPlanV2` enthält den ursprünglichen Auftrag, die Auswahl
`broker_controller` und die exakte `ExecutionInputRef`. Der Dienst erzeugt Akteur,
Controllerbindung und Kopierabsicht selbst. Ein bestätigter Plan verwendet
Version 2; Version-1-Hashes und Originalquittungen werden nicht umgeschrieben.

Vor neuer Annahme prüft der Dienst die aktuellen Originalgrants, Modelle,
Auftragsrevision, Eingaberetirement und Controllergeneration erneut. Erst nach
Bestätigung werden die eingefrorenen Bytes in eine private Namespace-Kopie
materialisiert. Die Queue erhält den Lauf zusammen mit der privaten Custody und
der unveränderten Originalquittung in einer Transaktion. Parallele Bestätigungen
und verlorene Antworten erzeugen keinen zweiten Lauf.

Eine öffentliche Controller-ID ist zufällig. Ihre private lokale Herkunft bindet
OS-Installation, natives State-Objekt, erwarteten kanonischen Verzeichnisanker
und einen privaten Marker. Kopieren oder Verschieben des Datenbestands erzeugt
keine neue Ausführungsberechtigung. Eine vorbereitete, aber nicht gestartete
Kopie wird nur mit derselben privaten Absicht und unveränderten Anfangsbytes
abgeglichen. Akzeptierte Custody stellt nach Neustart dieselben nativen
Arbeitsverzeichnisse wieder her und setzt bearbeitete Dateien nicht zurück.

Logische Ordnerkennungen bleiben für Rollen erhalten; Ressourcenkennungen der
Kopien entstehen ausschließlich aus dem neuen Namespace. Es gibt keinen
Rückfall auf Originalpfade. Rollen können Ordner und Schreibrechte einschränken.
Dateioperationen teilen über alle Aliase ein Namespace-Lock und prüfen Größe,
Einträge, temporär belegte Daten und das projizierte portable Ergebnismanifest
vor Veröffentlichung. Die festen Brokergrenzen sind 64 MiB je Datei, 512 MiB
sichtbarer Inhalt, 10.000 Einträge und 2 MiB Manifest. Ungeklärte Stages und
Tombstones bleiben belastet. Diese vertrauenswürdigen Brokerkontrollen ersetzen
keine begrenzten Runtime-Volumes und keine globale Kapazitätszulassung.

Einzelaufträge und Workflow-Rollen benutzen dieselben Kopien. Neue Modell- und
Dateiwirkungen prüfen die bestätigte Controllerbindung. Abbruch, Belege und
Ergebnisabgleich bleiben von einer neuen Ausführung getrennt. Ergebnisse werden
vor Abschluss als neue Inhaltsversion behalten; dieser Pfad übernimmt keine
Dateien automatisch in Originalordner.

Der Widerruf einer Eingabe oder ihrer ursprünglichen Ordnerfreigabe verhindert
neue Annahmen. Bereits zugelassene Kopien gehören zum angenommenen Auftrag;
deren weitere Ausführung wird über den Auftragsabbruch widerrufen. Ein bereits
ausgegebener Inhalt kann nicht durch Quellwiderruf zurückgeholt werden.

Die zusätzlichen Owner-Routen sind `POST /api/v2/execution-plans/v2`,
`POST /api/v2/execution-plans/v2/retire-request` sowie Inspect/Commit/Retire unter
`/api/v2/orders/{order}/execution-plans/v2/{plan}`. Authentisierte reine Leser
unter `/api/v2/orders/{order}/runs/{run}/working-copy/files` und `/text` binden
Auftrag, Lauf und gespeicherte Ordnerrechte; sie geben keine Hostpfade frei.
Der SDK prüft `canonical_json` gegen den öffentlichen Planhash, ohne Dezimalwerte
neu zu formatieren. Kleine lokale Bestätigungsreferenzen bewahren den Original-
vorgang über verlorene Startantwort und Neuladen hinweg.

Dieser Adapter startet keine allgemeinen Programme und nimmt keine MCP- oder
nativen Werkzeuganforderungen an. Ein fehlender Prozess-/Connectoradapter wird
konkret zurückgewiesen. App-eigene WSL2-/Linux-Runtime, entfernte Empfängerrechte,
reservierte Volumes, alle weiteren Startadapter und Originalübernahme bleiben
eigene offene Integrationen des Gesamtumbaus.

Der eigene PlanV2-/Arbeitskopiennachweis (development artifact omitted from public sources)
bindet ein unverändertes Quellenfenster und unsigned Entwicklungsartefakte.
Er umfasst die tatsächlichen Runtime-/HTTP-/SQL-/Brokerpfade mit synthetischem
Modell, 500 UI-Fälle, zehn Windows- und 26 Linux-Dateigrenzfälle sowie einen
eigenen PostgreSQL-16-Konkurrenzlauf. Die separate Browserprüfung behält ihre
früheren konkreten Binärbytes. Zwei echte lokale Modellversuche endeten am Limit
ohne verwertbaren Abschlussbeleg und sind ausdrücklich als fehlgeschlagen
gesichert. Reproduktion der Kernprüfung:

```powershell
python tools/v2_rebuild/qualify_execution_snapshots.py --wsl-distro Ubuntu-24.04 --postgres-report <aktueller-quellengebundener-PG-Bericht>
```

`--local-ollama` führt zusätzlich den ausdrücklich ausgewählten echten lokalen
Modellfall mit ausschließlich synthetischen Testdateien aus. Er lädt keine
Modelle herunter und verwaltet den bestehenden Modellservice nicht.

## Auswahl, Herkunft und Inhalt

`PrepareExecutionInput` enthält Auftragsrevision, Original-Idempotenzschlüssel und
sortierte, eindeutige `folder_ids` (höchstens 32). Hostpfade, Zugangsdaten, fremde
Manifeste oder vom Client erfundene Grants werden nicht angenommen. Der Runtime-
Resolver lädt den tatsächlichen Auftrag. Der FileBroker löst jede gewählte
Ressource gegen seine aktuelle Registrierung auf, prüft Projekt und gegebenenfalls
Auftrag und hält alle nativen Lesesperren in stabiler Reihenfolge bis zur
Custody-Transaktion. Eine bestätigte native Revocation kann die Erfassung nicht
überholen.

Jede Quelle erhält einen portablen Alias wie `input-0001`. Das öffentliche
`ExecutionInputSnapshot` enthält nur Snapshot-/Projekt-/Auftragskennung,
Auftragsrevision, Requesthash, Erfassungszeit, Anzeigenamen und das vollständige
[portable Inhaltsmanifest](WORKSPACE-TRANSFER.md). Keine nativen Ressourcen-
oder Device-/Inode-Kennungen verlassen die private Herkunftscustody. Ein leeres
Manifest ist eine ausdrücklich leere Eingabe und benötigt keinen FileBroker.

Die nativen `FolderGrant`-Baselines bleiben im privaten `CapturedExecutionInput`.
Migration 0110 speichert diese Herkunft getrennt von der öffentlichen Ansicht.
Öffentlicher Snapshothash, ursprünglicher Requesthash und privater Custodyhash
werden beim Laden unabhängig geprüft. Dateiinhalte werden im vorhandenen
FileBroker-Inhaltsbestand aufbewahrt; es entsteht keine zweite Projektverwaltung.

Nicht portable Eingabepfade werden bereits vor dem Zurückbehalten ihrer Bytes
abgewiesen. Symlinks, Windows-Junctions und fremde Projekt-/Auftragsressourcen
erzeugen keinen erfolgreichen Snapshot. Die bestehenden Grenzen für Einträge,
Dateigröße, Gesamtinhalt und Manifestgröße gelten auch hier. Das ist keine
Reservierung eines vollständigen Runtime-Dateisystems oder eines Transferpools.

## API und Wiederaufnahme

- `POST /api/v2/orders/{order}/execution-inputs` bereitet ausschließlich lokal vor.
- `GET /api/v2/orders/{order}/execution-inputs/{snapshot}` liest die ursprüngliche
  Metadatenquittung.
- `POST /api/v2/orders/{order}/execution-inputs/{snapshot}/retire` verwirft diese
  Vorbereitung mit ihrem exakten Snapshothash; die ursprünglichen Bytes bleiben
  als Historie erhalten.

Alle Routen erfordern die vorhandene Owner-Authentisierung im Header. Native
Erfassung teilt die bestehende Grenze von zwei blockierenden Dateiübertragungen.
Die tatsächliche I/O-Arbeit behält ihren Permit auch bei verlorenem HTTP-Aufrufer.
Quell-Lesesperren bleiben zusätzlich bis zum Custody-Commit gehalten.

Eine Wiederholung desselben Requests liefert dieselbe ursprüngliche Snapshot-ID
und denselben Hash, auch wenn Originaldateien inzwischen geändert wurden. Sie
erfasst diese Dateien nicht erneut. Konkurrierende Vorbereitungen werden beim
Custody-Commit am Auftrag serialisiert. Ein anderer Request unter demselben
Schlüssel wird abgewiesen. Retirement ist ein dauerhafter Tombstone und wird
durch Wiederholung nicht aufgehoben. Neue Erfassung verlangt einen neuen
ausdrücklichen Request. Geänderte Auftragsrevision vor einer ersten Annahme
verhindert die Veröffentlichung.

Der TypeScript-Client bietet `prepareExecutionInput`, `inspectExecutionInput` und
`retireExecutionInput`. Er prüft Manifest-/Snapshothash, Requestbindung,
Auftragsrevision und Originalquittung. Es gibt keine automatischen Wiederholungen
nach einer verlorenen Antwort. Für Eingaben existiert bewusst noch keine
öffentliche Chunk-/Byteausfuhrroute: Ein Hash oder Owner-Metadatenbeleg ist keine
an Plan und Empfänger gebundene Transferfreigabe.

## Vorheriger Nachweis der lokalen Eingabevorbereitung

Brokerprüfungen erfassen echte native Dateien und ein verwaltetes Arbeitsverzeichnis,
halten Widerrufssperren, übertragen die eingefrorenen Bytes trotz geänderter
Originale in den privaten Empfängerkern und materialisieren eine unabhängige Kopie.
Fremde, doppelte, nicht ausgewählte oder veraltete Quellen werden abgewiesen.
Windows prüft echte Junctions; Linux prüft Symlinks und nicht portable Namen.

Der gebundene frühere Abschlussnachweis (development artifact omitted from public sources)
enthält zehn Windows-Broker-/Storage-/HTTP-Fälle, 16 Linux-Brokerfälle, elf
gezielte SDK-Fälle und den aktuellen Vertrags-/TypeScript-Check mit 696 Typen.
1.191 Quelleninputs blieben im Prüfungsfenster unverändert. Ein späterer Delta
betrifft nur den separat kompilierten Integrationstest `workflow_waits.rs` und
ist im Entwicklungsartefakt ausdrücklich vermerkt. Drei unsigned Testbinaries,
der Review-Quellenbestand und ein unsigned Windows-Entwicklungsdaemon sind an
ihre konkreten Bytes gebunden; das Archiv wurde nach dem Packen vollständig gelesen.

Storagefälle prüfen 16 gleichzeitige Vorbereitungen, die Originalquittung nach
Retirement und tatsächlichem Datenbankneustart, konkurrierendes Verwerfen,
Auftragsänderung, Schlüsselkonflikt und beschädigte private Custody. Dieselben
Fälle sind gegen SQLite und einen eigenen temporären PostgreSQL-Cluster bestanden;
dieser Cluster wurde beendet und sein privater Dateisystembestand entfernt.
HTTP-Fälle prüfen echte Router-/Runtime-/Datenbank-/Brokerpfade mit nativen Dateien,
Authentisierung, Slotbegrenzung, Metadaten-Replay, fehlender Byteausfuhr und null
angelegten Läufen. Sie verwenden kein Modell und keinen Runner.

Reproduktion:

```powershell
python tools/v2_rebuild/qualify_execution_inputs.py --wsl-distro Ubuntu-24.04
```

Der separate PostgreSQL-Nachweis verwendet
`qualify_execution_plans_postgres.py --suite execution-inputs` mit einem expliziten
Cargo-Pfad und installiertem PostgreSQL innerhalb der Entwicklungsdistribution.
Er verwendet ausschließlich einen neu angelegten privaten Cluster und beendet
diesen anschließend. Produktionsdatenbanken werden nicht angesprochen.

Dieser ältere Nachweis umfasst ausschließlich die Vorbereitung. Die oben
beschriebene PlanV2-Bindung, das Queue-Gate und private Arbeitsversionen wurden
anschließend integriert und benötigen ihren eigenen Quellen-/Artefaktnachweis.
Offen bleiben aktuelle Remote-Empfänger-/Transfergrants, reservierte Runtime-
Volumes und das getrennte Baseline-/Konfliktreview für Änderungen an Originalen.
Die Vorbereitung allein erzeugt weiterhin keine Ausführungsautorität.
