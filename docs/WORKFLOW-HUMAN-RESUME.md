# Ausdrückliches Fortsetzen von Workflow-Rückfragen

**Begrenzte Human-Resume-Abnahme (9. Oktober 2026):** Die ausdrückliche Wiederaufnahme belegter nativer Workflow-Rückfragen ist im neuen unveränderlichen Sammelbeleg (private development evidence; omitted from public sources) an ihre exakten Quellen gebunden. Windows/Ubuntu bestehen 387/370 Rusttests mit Format und Clippy, 14 tatsächliche PostgreSQL-Verträge, je zwölf Daemon-Prozesssuiten und 178 Frontendtests samt TypeScript, 163 Verträgen und Build. SQL 20/private Clock 1 bleiben erhalten; neue Continuations verwenden Progress 4, Ereignis 8 und Export 13. Resume und Accepted-Replay senden weder alte Modellanfragen noch gespeicherte Antworten. Antworten benötigen einen eigenen ausdrücklichen Commit. Der begrenzte Windows-Browserbeleg prüft Resume, Reload, Quittierung und separates Antwort-Commit; seine Fixture wurde extern unterbrochen, anschließend wurde ihre aktuelle Prozess-/Port-Abwesenheit geprüft. Kein historischer Browser-Reap oder Thread-Join wird daraus abgeleitet. Frühere Belege bleiben unverändert. Hierarchie, unbekannte Wirkungen, allgemeiner Resume, native GUI, NVDA/WCAG, Enterprise, vollständiges M5/M0–M10 und Release bleiben offen. Siehe [Ablauf und Grenzen](WORKFLOW-HUMAN-RESUME.md).

Stand: 9. Oktober 2026. Dieser Schnitt gilt für die persönliche
`single_owner_preview`-Ausführung nativer sequenzieller und paralleler Workflows.
Der neue Abnahmebeleg (private development evidence; omitted from public sources)
und der exakte Quellsnapshot (private development evidence; omitted from public sources)
sind separat erhalten. Die früheren Clock-, Input- und Layoutbelege bleiben unverändert.

Die tatsächliche Browserfolge war ausdrücklicher Resume → Reload → ausdrückliche Ergebnisquittierung → separater Antwort-Commit. Der ursprüngliche Bericht bleibt erhalten; seine zusammengefasste Reload-/ACK-Formulierung wird im Sammelbeleg präzisiert. Die unterbrochene Browserfixture besitzt nur den gesonderten Nachweis ihrer anschließend geprüften Prozess-/Port-Abwesenheit, keinen historischen Reap-/Thread-Join-Beleg. Der zusätzliche PostgreSQL-Abschlussabgleich (private development evidence; omitted from public sources) bestätigt den positiven Linux-Postmaster-Abschluss und die entfernte private Fixture. Die zusätzliche Windows-Launcher-Quittung fehlt; aktuelle Abwesenheit belegt keinen historischen Windows-Wait/Reap.

Nach einer Unterbrechung bietet der Dienst „Rückfragen fortsetzen“ nur an, wenn
der gesamte Graph dafür geeignet ist. Jede unterbrochene aktive Node braucht
eine belegte Frage am letzten gespeicherten, bestätigten Modellturn. Unbekannte
Modell- oder Werkzeugwirkungen, endgültig geschlossene Fragen, Widerruf,
tatsächlicher Abbruch, erschöpfte Budgets und fehlende oder gesperrte private
Clocks verhindern die Wiederaufnahme. Hierarchische Ausführung bleibt gesperrt.
Eine Vorschau ist keine Ausführungsberechtigung; der Dienst prüft beim Commit
und bei jeder späteren Wirkung erneut.

Die Oberfläche bereitet nach einem ausdrücklichen Klick einen dauerhaften
`ResumeInput`-Vorgang vor. Er bindet den ursprünglichen Lauf, beide
Basisrevisionen sowie Checkpoint- und Cursorhash. Der durch denselben ausdrücklichen Klick beauftragte anschließende
Commit übernimmt diesen unveränderlichen Vorgang, den Elternzustand `Queued`,
die vorbereitete Generation und den tatsächlichen Annahmebeleg in derselben
SQL-Transaktion. Ein verlorenes ACK wird über denselben Vorgang abgeglichen;
ein Accepted-Replay liest das originale Receipt und stellt nichts erneut an.

Der reguläre Queueworker aktiviert ausschließlich diese vorbereitete Generation
in einer zweiten Transaktion. Neue Eltern-Lease und neue Ausführungsclaims
werden gemeinsam mit dem Aktivierungsereignis gespeichert. Der ursprüngliche
Modellclaim, der Besitzer des historischen Humanattempts und die aktuelle
Ausführungsberechtigung bleiben getrennt. Alte Leases werden nicht wieder gültig.
Journalreceipt, Reducerreplay, gespeicherte Frage-/Aktivierungsereignisse und
Claims werden auch bei nachfolgenden Wirkungen geprüft.

Ziel, Auftrag, Projekt, Workflowversion, Modellprofile, Executor, Rechte,
Originalstart, Fristen und bisherige Reservierungen bleiben erhalten. Warten,
Neustart und Fortsetzen erweitern kein Budget. Tatsächliche Lockwartezeiten
werden vor dem Commit erneut gegen die ursprünglichen absoluten Fristen geprüft.
Die historische Clock-Ursprungsprojektion bleibt Format 1 und behält ihre
Serialisierung; neue Continuations erzeugen erst bei ihrer typed Entscheidung
StoredWorkflow/Progress 4. SQL bleibt 20 und private Clock 1; aktuelle Ereignisse
verwenden Schema 8 und der Gesprächsexport Format 13.

Der Worker liest den bestätigten alten Modellturn lokal. Er sendet keine alte
Modellanfrage erneut. Gesicherte Antwortentwürfe und bereits vorbereitete
Antwortintents bleiben erhalten. Öffnen, Polling, Neuverbinden, Queueaktivierung
und Accepted-Replay senden keine Antwort. Jede Antwort braucht weiter ihren
eigenen ausdrücklichen Commit. Weitere erlaubte Datenbankcalls erhalten die
reguläre aktuelle Policy-/Freigabeprüfung. Cursorverbrauch und eine anschließende
neue Modelladmission samt neuer Tokenreservierung erfolgen atomar.

Entzug, Abbruch, Fristablauf oder Clockablehnung nach Accepted führen zu einer
eng gebundenen, idempotenten Nichtaktivierung. Das Accepted-Receipt bleibt
historisch gültig, Charges bleiben erhalten und terminale Elternzustände werden
nicht überschrieben. Dieser gesonderte Abschluss erweitert weder die bestehende
Cancellation- noch die Recovery-Ausnahme und repariert keine alte Clock.

Der Export enthält die ursprünglichen Resumeoperationen einschließlich ihrer
Belege. Ein Export verleiht keine Ausführungsrechte. Weitere sichere allgemeine
Wiederaufnahme, externe Workflow-Datei-/MCP-/Prozesswirkungen, Hierarchie,
Enterprise-/Offline-Rechte, native GUI-/Barrierefreiheitsabnahme, vollständiges
M5 und der Gesamtplan M0–M10 bleiben offen. Produktive Umstellung und Release
sind nicht freigegeben.
