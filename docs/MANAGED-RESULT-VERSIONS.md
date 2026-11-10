# Private Ergebnisversionen registrierter Owner-Runner

Der Runtime-Kern kann das Ergebnis eines ursprünglich abgeschlossenen Managed-
Prozessversuchs als getrennte lokale Version empfangen und vollständig prüfen.
Er verändert dabei weder Originaldateien noch die bestätigte Arbeitskopie und
startet keinen weiteren Prozess. Öffentliche Owner-Routen und die sichtbare
Ergebnisprüfung sind inzwischen im [Owner-Anschluss](MANAGED-OWNER-RESULTS.md)
integriert und im aktuellen Frontend-/SDK-Prüfstand geprüft.

`Runtime::retain_managed_result_version` verwendet den vorhandenen Plan, Lauf,
Eingabetransfer und die originale signierte Abschlussquittung. Vor der ersten
nativen Reservierung speichert Schema115 eine unveränderliche Empfangsabsicht
in derselben Controller-Datenbank. Sie bindet auch das ursprüngliche signierte
Ausgabemanifest und dessen feste Aufbewahrungsfrist. Es entsteht keine zweite
Projekt- oder Ausführungsdatenbank.

Verlorene Antworten werden über dieselbe Version fortgesetzt. Ohne bereits
gespeicherte Custody darf nur die exakt passende abgeschlossene ursprüngliche
native Vorbereitung wiedergefunden werden. Nach gespeicherter Custody verlangt
der Ablauf deren unabhängige ursprüngliche Verzeichnis- und Receiveridentitäten.
Unvollständige Vorbereitung, Ersatzverzeichnisse oder beschädigte Packdaten
führen zur Ablehnung; es gibt keine Rekonstruktion aus Originalen.

Der Empfang nutzt den [begrenzten nativen Empfänger](MANAGED-RESULT-STAGES.md).
Nur deklarierte fehlende Blöcke werden einzeln abgerufen und geprüft. Die
Runtime löst [aktuelle Leserechte](MANAGED-RESULT-READS.md) für jeden Abruf erneut
auf. Ein neuer Versuch nach einem Übertragungsfehler ist ausdrücklich nötig;
er verwendet die ursprüngliche Version und lädt ausschließlich fehlende Blöcke.

`Ready` wird erst nach vollständiger nativer Dateiprüfung und einer erneuten
SQL-Autorisierungsprüfung veröffentlicht. Eine echte, nicht serialisierbare
Verifikationsfähigkeit hält Receiver, Stage und ursprünglichen Payload-Handle
bis über den Commit. Ein gleich aussehender Datensatz ersetzt diese Fähigkeit
nicht. Native Scans laufen außerhalb der SQL-Transaktion.

`Runtime::managed_result_version_chunk` liest anschließend deklarierte Blöcke
aus der geprüften lokalen Version. Die ursprüngliche Remote-Aufbewahrungsfrist
wird dabei nicht verlängert; bereits erworbene lokale Bytes bleiben unabhängig
von deren Ablauf erhalten. Der lokale Ready-Pfad prüft aktuelle Owner-, Auftrags-,
Controller-, Kopie- und Quellrechte sowie die ursprüngliche Plan-, Ziel- und
Abschlussbindung. Er verlangt keine aktuelle Ausführungsbereitschaft des Runners
oder der Umgebung. Empfang und Ready-Veröffentlichung behalten dagegen sämtliche
frischen Remote-Prüfungen. Der separate Nachweis (development artifact omitted from public sources)
besteht 46 SQLite- und 46 getrennte PostgreSQL-Fälle auf demselben neuen Artefakt,
darunter sechs neue lokale Ready-Fälle. Der unten verlinkte 40/40-Prüfstand bindet
den vorherigen Stand; alle 46 zusätzlichen Prüfdatenbanken wurden entfernt.
Ein Broker-Neustart öffnet ausschließlich
den unabhängig gespeicherten ursprünglichen Quell-Workspace wieder. Fehlende
oder ausgetauschte Quellen werden nicht neu angelegt oder übernommen.

Der quellgebundene Prüfstand (development artifact omitted from public sources) besteht
40 SQLite- und 40 getrennte PostgreSQL-Fälle mit demselben erfassten Testartefakt,
darunter 13 neue Ergebnisversionsfälle. Alle zugehörigen Testdatenbanken wurden
entfernt und der Katalog geprüft. 19 Windows- und 20 WSL/Linux-Dateisystemfälle
prüfen den Empfänger, davon je zehn Stage-Fälle. Sechs Migrationsregressionen
und die echte Migration einer aufbewahrten Schema114-Kopie nach115 erhalten
400 bestehende Tabellendefinitionen samt Inhalt und alte Migrationsquittungen.
Die ursprüngliche Datenbank und das aktive App-Profil bleiben unverändert.

Vorherige Nachweise behalten ihren eingefrorenen Quellenumfang. Gescheiterte
Prüfläufe und die Unterbrechung durch den Laptop-Neustart sind getrennt erhalten.
Große verschachtelte Futures liegen nun im Heap; die normale Thread-Stackgröße
bleibt bestehen. Zwei später gefundene PostgreSQL-Testaufbaufehler wurden nur
im Prüfaufbau korrigiert: eine zu kurze synthetische Aufbewahrungsfrist und eine
zweite Controller-Identität für dieselbe Datenbank. Die Produktionsprüfungen
wurden dafür nicht gelockert.

Die öffentlichen Metadaten-/Abrufrouten, Client und Ergebnisoberfläche sind
angeschlossen. Weiter offen sind die aktuelle vollständige App-Abnahme,
ausdrückliche Vergleichs-/Übernahmeprüfung mit Schutz lokaler Änderungen,
dauerhafte Kapazitätsfreigabe sowie der tatsächliche native HTTPS-/App-Ablauf.
Die aktive lokale Entwicklungs-App wurde durch diese Quellarbeit nicht aktualisiert.

Der Owner-Anschluss verwendet eine öffentliche Projektion der SQL115-Version
ohne native Custody oder private Protokollobjekte. Vergleichsbasis für die noch
fehlende Übernahme ist das genaue
ursprüngliche Eingabemanifest. Der bestehende `FileRestoreSource` verweist bisher
auf Toolquittungen und benötigt eine ausdrücklich typisierte Managed-Quelle mit
Version, Revision, Manifest und ausgewähltem Pfad. Das erste Übernahmeziel bleibt
eine neue bestätigte Kopie mit fehlendem Zielpfad; bestehende lokale Dateien
brauchen vor einer späteren Ersetzung zusätzlich ihre erwartete aktuelle Identität
und ihren Hash. Es werden keine künstlichen Toolquittungen erzeugt.

Auch der vorhandene Branch-Pfad übernimmt derzeit nur `workspace_snapshot`.
Eine Managed-Ergebnisaktion darf diesen Pfad erst anbieten, wenn die ausdrücklich
gewählte Version mit tatsächlich verifizierten Bytes die neue unabhängige
Workspacebasis bestimmt. Die bestehende Originalquittung und Idempotenz bleiben
dabei maßgeblich.
