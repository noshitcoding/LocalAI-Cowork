# Verwaltete Arbeitsdateien

Jeder Auftrag hat ein eigenes dauerhaftes Arbeitsverzeichnis. Der lokale Daemon legt es
bei der ersten Verwendung unter seinem privaten Datenverzeichnis an. Weitere Läufe
verwenden denselben Dateibestand. Persönliche Host-Verzeichnisse werden dadurch weder
freigegeben noch importiert.

## Zuständigkeit und Rechte

Die Anwendung registriert die Workspace-Freigabe in SQLite beziehungsweise PostgreSQL.
Die Freigabe bindet Auftrag, Projekt, Ausgangssnapshot und physische Verzeichnisidentität.
Der Broker prüft diese Bindung bei jedem Zugriff. Eine fremde Workspace-Freigabe kann
nicht als zusätzliche Ordnerauswahl eingeschleust werden. `/folders` enthält weiterhin
nur ausdrücklich konfigurierte externe Verzeichnisse.

Neue Läufe erhalten ihre eigene Workspace-Freigabe in der eingefrorenen Konfiguration.
Dateiwerkzeuge laufen durch denselben Broker, dieselben vier Zugriffsmodi und dieselbe
argumentgebundene Freigabe wie externe Dateien. Schreibende Läufe beanspruchen den
dauerhaften Dateibesitz. Alte bereits angenommene Läufe werden nicht nachträglich um
Workspace-Rechte erweitert.

Der Dateibestand wird über `GET /api/v2/orders/{order}/workspace`, `/workspace/files?path=.`
und `/workspace/text?path=…` angezeigt. Diese Endpunkte sind für den lokalen Besitzer
mit dem privaten Daemon-Token geschützt. Organisationsautorisierung und Serverbetrieb
sind weiterhin separate, noch nicht implementierte Meilensteine.

## Abschluss und Wiederaufnahme

Eine abschließende Modellantwort wird zunächst mit `completion_pending` und dem Zustand
`waiting_for_snapshot` bestätigt. Der Broker liest den Dateibaum unter einer OS-Lesesperre,
bewahrt Dateiinhalte nach SHA-256 auf und erzeugt ein sortiertes Manifest. Laufabschluss,
abschließende Nachricht, Ereignis und Snapshot werden gemeinsam in einer Datenbanktransaktion
gespeichert. Erst danach wird der dauerhafte Schreibbesitz freigegeben.

Die Snapshot-Phase besitzt eine erneuerte Ausführungs-Lease. Nach Lease-Verlust bleibt
der Lauf unterbrochen. Eine ausdrückliche Fortsetzung verwendet die bereits gespeicherte
Modellantwort; sie fragt kein Modell erneut und wiederholt keine Dateiänderung.
Unbestätigte schreibende Werkzeuge müssen weiterhin zuerst abgeglichen werden.

## Verzweigungen

Eine Verzweigung referenziert den Snapshot der ausgewählten abschließenden Nachricht.
Spätere Änderungen im Ursprungsauftrag verändern diesen Stand nicht. Auch eine weitere
Verzweigung aus einem übernommenen historischen Verlauf behält diese Zuordnung.

Die neue Arbeitskopie wird zunächst in einem privaten Zwischenverzeichnis aufgebaut.
Alle Inhalte werden anhand des Manifests geprüft, in eigenständige Dateien kopiert und
anschließend als vollständiges Verzeichnis veröffentlicht. Hardlinks zwischen Arbeitskopien
werden nicht verwendet. Wiederholte Anfragen ersetzen keine späteren Änderungen im Zweig.

Fehlende oder beschädigte Inhalte verhindern die Materialisierung. Ein bereits registriertes,
verschwundenes oder ausgetauschtes Arbeitsverzeichnis wird nicht als leeres Verzeichnis
neu angelegt. Ein vor Veröffentlichung abgebrochener Aufbau kann unreferenzierte private
Zwischendaten hinterlassen; diese werden nicht automatisch als gültige Arbeitskopie übernommen.
Eine geregelte Bereinigung und Speicherkontingente stehen noch aus.

Historische Läufe aus älteren V2-Preview-Ständen besitzen keinen Dateisnapshot. Für deren
Verzweigungen entsteht ein leeres Arbeitsverzeichnis. Es wird kein heutiger Dateistand als
historischer Stand ausgegeben. Externe Ordner werden grundsätzlich nicht mitverzweigt.

## Geprüfte Dateidownloads

Die Ergebnisansicht bietet getrennte Downloads für aktuelle Arbeitsdateien und den
Dateistand des ausgewählten Laufs. Unterstützt werden reguläre Text- und Binärdateien
bis 64 MiB einschließlich leerer Dateien und Unicode-Dateinamen.

- `GET /api/v2/orders/{order}/workspace/download?path=…` liest den aktuellen Bestand
  unter der gemeinsamen OS-Lesesperre. Ein optionaler `expected_sha256` verhindert,
  dass ein inzwischen geänderter Inhalt als die angezeigte Vorschau heruntergeladen wird.
- `GET /api/v2/orders/{order}/runs/{run}/workspace/download?path=…` bindet Lauf und
  Auftrag und liest ausschließlich den bestätigten Snapshot. Der Pfad muss darin als
  reguläre Datei enthalten sein. Manifest, Größe und aufbewahrter Inhalt werden geprüft;
  eine fehlende oder beschädigte Version fällt niemals auf die aktuelle Datei zurück.
- Beide Endpunkte verlangen den Daemon-Token im Authorization-Header. Pfadgrenzen,
  Linkverbote und die Dateigrößenbegrenzung gelten auch für Downloads. Es gibt keine
  öffentlichen Datei-URLs und keine geheimen Zugangsdaten in Query-Parametern.
- Die Antwort verwendet `application/octet-stream`, `Content-Disposition: attachment`,
  eine percent-kodierte UTF-8-Dateibezeichnung, `Content-Length` und `X-Cowork-Sha256`.
  CORS gibt nur die benötigten Metadaten für bereits erlaubte Ursprünge frei.
- Der gemeinsame Client begrenzt den Empfang auf die deklarierte Größe und prüft den
  tatsächlichen SHA-256 vor der Übergabe an die Browser-Dateiablage. Falsche Header,
  abgeschnittene Antworten, Mehrdaten und manipulierte Hashes erzeugen einen Fehler.

Der Client lädt eine Datei vollständig in den Speicher. Die Grenze von 64 MiB gilt pro
Download; organisationsweite Transferquoten und große gestreamte Downloads sind noch
nicht implementiert. Verzeichnisarchive und beliebige externe Ordnerdownloads fehlen.
Die Web-Oberfläche bestätigt nur die geprüfte Vorbereitung. Im eingebetteten Prüfbrowser
ließ sich kein Download-Ereignis und keine Datei am erwarteten Standard-Downloadpfad
nachweisen. Desktop verwendet inzwischen einen nativen Speicherdialog mit Rust-Dateischreiber,
Prüfsummenvergleich und getrenntem Abbruchzustand für Dateien, Artefakte und JSON-Exporte.
Windows-Dateisystem- und Prozessabbruchtests bestehen; die vollständige interaktive
Browser-/Desktop-Dateiablage ist weiterhin nicht abgenommen. Siehe NATIVE-SAVE.md.

## Format und derzeitige Grenzen

- Datenbankschema 4 ergänzt Workspace-Registrierungen und unveränderliche Lauf-Snapshots.
- Der aktuelle Auftragsexport Version 7 enthält Dateimanifeste, Upload-Metadaten, Modellbudget-Belege und Herkunft; Datei-Blobs sind darin nicht
  enthalten. Dieser Gesprächsexport ersetzt kein Backup oder Migrationspaket.
- Alte Historienhashes bleiben gültig, wenn noch keine Workspace-Snapshots enthalten waren.
- Snapshots unterstützen reguläre Binär- und Textdateien, leere Verzeichnisse, höchstens
  64 MiB pro Datei, 512 MiB insgesamt, 10.000 Einträge und 64 Verzeichnisebenen.
- Links, Hardlinks, Spezialdateien, unzulässige Pfade und inkonsistente Inhalte werden abgewiesen.
- Die interaktive Textvorschau und das Textwerkzeug bleiben auf 1 MB UTF-8 begrenzt.
- Binär-Upload und expliziter Import einschließlich Ersetzen mit Vorversion sind in FILE-UPLOADS.md beschrieben.
- Verzeichnisarchive, Paketumgebungen und interaktive Terminals sind noch nicht
  implementiert; die native Speicherimplementierung benötigt weitere Abnahme. Anlegen, Verschieben, Entfernen
  und Wiederherstellen sind in FILE-OPERATIONS.md beschrieben.
- Der Daemon ist weiterhin keine qualifizierte Sandbox für beliebige native Programme.
  Gegenüber externen Editoren sind die Sperren kooperativ. NTFS-Prozessabbruch und Linux-
  Prozessabbruch sind geprüft; Stromausfall, ReFS und Netzwerkdateisysteme sind nicht abgenommen.

## Nachweise

`crates/broker/tests/workspaces.rs` prüft physische Identität, binäre Kopien, getrennte Zweige,
unzulässige Pfade, fehlende Verzeichnisse und beschädigte Inhalte. Runtime-Tests prüfen
ältere Zweiggrenzen, übernommene Historie und Wiederaufnahme ohne verfügbaren Provider.
Der gemeinsame SQLite-/PostgreSQL-Vertrag prüft die atomare Speicherung und Export-Herkunft.

`tests/system/workspaces.py` startet echte Daemon-Prozesse mit einem lokalen HTTP-Testprovider.
Ein ausschließlich in der Wegwerf-Datenbank installierter Trigger lässt den Snapshot-Insert
scheitern. Nach Neustart und ausdrücklicher Fortsetzung werden weder Modellanfrage noch
Dateischreiben wiederholt. Zusätzlich werden Binärdateien, Zweigisolation und unabhängige
Manifesthashes geprüft. Die erweiterte Windows-Prüfung lädt aktuelle und historische
Binärdateien nach Neustart, weist fremde Aufträge und veraltete Vorschau-Hashes ab und
verweigert beschädigte gespeicherte Inhalte. Die Linux-Ausführung dieser Erweiterung
steht noch aus. Dies ist kein Nachweis realer Modellleistung.
