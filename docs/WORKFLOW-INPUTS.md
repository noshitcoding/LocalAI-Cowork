# Dauerhafte Rückfragen in nativen Workflows

**Begrenzte Human-Resume-Abnahme (9. Oktober 2026):** Die ausdrückliche Wiederaufnahme belegter nativer Workflow-Rückfragen ist im neuen unveränderlichen Sammelbeleg (private development evidence; omitted from public sources) an ihre exakten Quellen gebunden. Windows/Ubuntu bestehen 387/370 Rusttests mit Format und Clippy, 14 tatsächliche PostgreSQL-Verträge, je zwölf Daemon-Prozesssuiten und 178 Frontendtests samt TypeScript, 163 Verträgen und Build. SQL 20/private Clock 1 bleiben erhalten; neue Continuations verwenden Progress 4, Ereignis 8 und Export 13. Resume und Accepted-Replay senden weder alte Modellanfragen noch gespeicherte Antworten. Antworten benötigen einen eigenen ausdrücklichen Commit. Der begrenzte Windows-Browserbeleg prüft Resume, Reload, Quittierung und separates Antwort-Commit; seine Fixture wurde extern unterbrochen, anschließend wurde ihre aktuelle Prozess-/Port-Abwesenheit geprüft. Kein historischer Browser-Reap oder Thread-Join wird daraus abgeleitet. Frühere Belege bleiben unverändert. Hierarchie, unbekannte Wirkungen, allgemeiner Resume, native GUI, NVDA/WCAG, Enterprise, vollständiges M5/M0–M10 und Release bleiben offen. Siehe [Ablauf und Grenzen](WORKFLOW-HUMAN-RESUME.md).

Stand: 9. Oktober 2026. Dieses Dokument beschreibt den eingefrorenen qualifizierten
persönlichen Input-/Layoutstand mit Datenbank 19, Ereignisschema 7, Export 12 und
161 generierten Verträgen. Die begonnene private Clock-/SQL-20-Erweiterung verändert
inzwischen den Kern und ist hier nicht abgenommen. Die Nachweise unten bleiben an
ihre archivierten Quellen gebunden; sie erteilen keine Gesamtfreigabe für M5,
den Neubau, Enterprise, native Desktop-GUI, Barrierefreiheit oder Release.

Der frühere [Inputentwurf](WORKFLOW-INPUT-DESIGN.md) beschreibt die damaligen
Quellbefunde und weitergehende Ziele. Für das qualifizierte 19/7/12-Verhalten
gilt dieses Dokument. Insbesondere ist die dort entworfene Wiederaufnahme eines
unterbrochenen menschlichen Wartecursors **noch nicht verfügbar**.

## Fragen und Agentenkontext

Die Produktanbindung unterstützt sequenzielle und parallele Modelle sowie genau
vier Werkzeuge: `create_artifact`, `list_artifacts`, `read_artifact` und
`request_input`. Das neue Werkzeug akzeptiert ausschließlich ein Objekt mit dem
Feld `question`. Die Frage muss Text enthalten, darf kein NUL enthalten und ist
auf 4.000 UTF-8-Bytes begrenzt.

Eine Frage entsteht ausschließlich aus dem nächsten dauerhaft bestätigten
Werkzeugvorschlag eines Modellturns. Ihre Broker-ID bindet Lauf, Knoten, Turn und
rohe Provider-Call-ID. Gleichnamige Call-IDs verschiedener Agenten oder späterer
Turns erzeugen dadurch getrennte Fragen. Der zusätzliche Fingerprint bindet
Projekt, eingefrorene Workflowversion, Rolle, vollständigen Werkzeugvorschlag,
Ausführungsziel, Policyrevision, Zugriffsmodus und den jeweiligen Werkzeug- und
Dateiscope. Fremde oder veränderte Bindungen werden nicht als neue Antwort
interpretiert.

`request_input` durchläuft denselben Policypfad wie die Artefaktwerkzeuge und gilt
als lesende Aktion. Im Modus „Jede Aktion bestätigen“ wird zuerst die exakte
Werkzeugfreigabe verlangt. Eine menschliche Antwort erzeugt keine neuen Rechte
und ersetzt keine Werkzeugfreigabe.

Während einer Frage bleibt der Parent-Lauf `running`; seine Lease und Heartbeats
bleiben erforderlich. Der betroffene Agent wartet in seinem eigenen belegten
Workflowplatz. Andere zugelassene Geschwister können weiterarbeiten. Im View wird
der betroffene Knoten als `waiting_input` angezeigt; der Parent wechselt nicht
in den Einzelagentenstatus `waiting_input`.

Eine bestätigte Antwort gelangt als Werkzeugresultat ausschließlich in den
Verlauf des fragenden Agenten. Der exakte Text wird dem nächsten Modellaufruf
übergeben, ohne Kürzung, Trimmen oder JSON-Zeichenkettenumwandlung. Parentprompt
und Geschwisterverläufe erhalten keine Antwortkopie. Die nächste Modellfortsetzung
beginnt erst, wenn die gespeicherten Werkzeugvorschläge dieses Turns abgeschlossen
sind. Mehrere unabhängige Fragen können separat und gleichzeitig beantwortet
werden; die gemeinsame Graphrevision schützt vor verlorenen Aktualisierungen.

## Antwortentwürfe und unveränderliche Snapshots

Antwortentwürfe gehören dienstseitig zum festen Owner `single_owner_preview`.
Der Browser darf keinen Owner wählen. Lauf, Projekt, Frage und Fingerprint werden
bei Lesen und Schreiben geprüft. Dies ist eine persönliche Vorschau und **keine
Enterprise-Identität oder organisationsbezogene Inhaltsautorisierung**.

Ein noch ungespeicherter Entwurf wird als Revision 0 mit leerem Text und ohne
Snapshot-ID gelesen. Entwürfe dürfen leer sein oder nur Leerraum enthalten.
Jeder Text ist auf 32.000 UTF-8-Bytes begrenzt; NUL ist verboten. Erst die
Vorbereitung einer verbindlichen Antwort verlangt einen nicht leeren Textinhalt.
Eine erfolgreiche Speicherung verändert den Text nicht.

Speichern verwendet Basisrevision und Idempotenzschlüssel. Bei passender Basis
wird Revision `base_revision + 1` bestätigt und ein unveränderlicher Snapshot
angelegt. Eine veraltete Basis erzeugt HTTP 409 mit `outcome: conflict`: Der
bisherige Head bleibt bestehen und der eingehende Text wird als unveränderliche
Konfliktkopie gespeichert. Diese Kopie erhält die damals bestätigte Headrevision
und behält ihre eingehende Basisrevision. Der Konflikt ist somit ein bestätigter
Speicherbeleg und kein Anlass, den Text zu verwerfen.

Ein erneuter Request mit demselben Schlüssel und identischem Inhalt liefert den
ursprünglichen Beleg einschließlich der damaligen Snapshots. Eine spätere
Headänderung ersetzt diesen Beleg nicht. Derselbe Schlüssel mit verändertem
Payload wird verweigert. Interne Speicherquittungen verwenden geprüfte
Snapshotreferenzen, statt große Textkopien pro Quittung zu vervielfachen.

GET des Entwurfs liefert den bestätigten Head, alle begrenzten Snapshotmetadaten
und alle Antwortvorgangsmetadaten dieser Frage. Ein konkreter Snapshot wird über
seine ID gelesen und auf Scope, Revision, Metadaten und Texthash geprüft. Die
Oberfläche zeigt historische oder konfliktbehaftete Texte zunächst zur Prüfung;
eine Übernahme wird eine ausdrückliche lokale Bearbeitung mit einer neu gewählten
bestätigten Basis. Sie überschreibt den Head nicht automatisch.

Bei `interrupted` oder einer unklaren Lease dürfen noch offene, nicht abgelaufene
Entwürfe weiter gesichert werden. Beantwortete, entzogene, abgelaufene oder aus
einem bekannten Knotenausgang geschlossene Fragen bleiben lesbar, erlauben aber
keine neue Antwortvorbereitung. Speichern selbst erteilt keine Ausführungsrechte.

## Vorbereiten und ausdrücklich bestätigen

Der Bedienablauf besteht aus drei getrennten Entscheidungen:

1. Text bearbeiten und als Entwurf speichern.
2. Aus genau der bestätigten aktuellen Entwurfsrevision einen Antwortvorgang
   vorbereiten. `PrepareWorkflowInputAnswer` enthält nur Fingerprint,
   `base_draft_revision` und Idempotenzschlüssel. Der Dienst liest den Text und
   Snapshot selbst und friert die vollständige ursprüngliche Antwort ein.
3. Diesen gespeicherten Vorgang ausdrücklich bestätigen. Commit erhält nur die
   Vorgangs-ID im Pfad und **keinen Requestbody**, auch kein `{}`.

Ein Vorgang besitzt `pending`, `accepted` oder `rejected` als Zustand. Er enthält
die unveränderliche Submission, Snapshotreferenz und Inhalts-/Requesthashes;
spätere Entwurfsänderungen ändern ihn nicht. Ein noch ungeklärter Pending-Vorgang
blockiert eine neue Vorbereitung derselben Frage. Derselbe ursprüngliche
Prepare-Schlüssel darf weiter abgeglichen werden. Ein neuer Schlüssel für einen
veränderten Text ist erst nach geklärtem Vorgang und ausdrücklicher Auswahl
zulässig. Es gibt in diesem Schnitt weder Ack- noch Discard-Routen für Antworten.

Nach einem verlorenen Prepare-Erfolg findet GET des Entwurfs alle gespeicherten
Vorgangs-IDs wieder. Der vollständige Vorgang wird anschließend über seine eigene
GET-Route gelesen. Metadaten werden nicht zur Rekonstruktion eines Committexts
verwendet. Nach verlorenem Commit-Erfolg liefert Lesen oder Wiederholen derselben
ID den gespeicherten Zustand. Die Oberfläche erzeugt keinen Ersatzschlüssel und
sendet keine Antwort automatisch beim Wiederöffnen, Polling oder Sitzungswechsel.

Ein angenommenes Replay wird vor erneuten Live-Lease-, Policy- oder Fristprüfungen
zurückgegeben, nachdem Scope und Belegintegrität geprüft sind. Es ist eine
historische Bestätigung und erzeugt keine zweite Antwortwirkung. Ein aktuell
entzogener Workflow macht einen bereits angenommenen Beleg nicht rückwirkend
zu einem neuen Vorgang.

## Atomare Wirkung und letzte Zulassungsprüfung

Öffnen und Beantworten sind eigene replayprüfbare Workflowentscheidungen. Beim
Commit werden Graphfortschritt, Agenten-Werkzeugreceipt, menschliches
Antwortreceipt, das Ereignis `workflow_input_answered` und der angenommene
Antwortvorgang in **derselben Datenbanktransaktion** geschrieben. Es gibt keine
Antwortwirkung mit nachträglich getrennter Bestätigung des Intentjournals.

Das öffentliche Receipt bindet Request-ID, Fingerprint, Receipt-ID,
`answer_sha256` und die bestätigte Ereignissequenz. Lesen und Export prüfen nicht
nur einen positiven Sequenzwert, sondern das tatsächlich gespeicherte Ereignis
des Laufs sowie die identischen Workflow- und Intentreceipts. Umgeschriebene
Payloads oder unpassende Zeilenmetadaten werden verweigert.

Die Zulassung prüft den aktuellen eingefrorenen Knotenscope, exakte Freigaben,
Workflowentzug, Stopzustand, Parent-Lease und die kleinste Frist aus Parent,
Knoten und Vorfahren. Registry- und Runzustand werden nach Lockwartezeiten erneut
gelesen; Lease und Frist bleiben bis zur letzten Commitprüfung gefenced. Auch
Entwurfsspeicherung und Prepare prüfen die verbleibende Schreibbarkeit unmittelbar
vor dem Commit. Provider-I/O findet nicht in diesen SQL-Transaktionen statt.

Bei konkurrierenden Antworten wird gegen den neuesten Workflowcheckpoint erneut
reduziert, während der ursprüngliche eingefrorene Antwortvorgang unverändert
bleibt. Sichere Datenbankkonflikte können nach zurückgerollter Transaktion neu
versucht werden; eine angenommene Antwort gewinnt gegen eine verspätete Ablehnung.

Bekannter Abbruch, Ablauf, Entzug, Workflow-Stop oder eine definitiv geschlossene
Frage erzeugen einen terminalen `rejected`-Beleg ohne Antwortwirkung. Eine bloße
Unterbrechung oder ungeklärte Lease erzeugt diese endgültige Ablehnung nicht:
Der Vorgang bleibt `pending`, und der Fehler bleibt sichtbar. Ein zuvor tatsächlich
bestätigtes `cancel_requested`-Ereignis bleibt dagegen auch nach einer konservativen
Unterbrechung verbindlich. Unter den Registry-/Runlocks werden seine gespeicherte
Zeile, Lauf-ID, Sequenz und der exakte Abbruchpayload geprüft. Zusätzlich muss der
gesamte gespeicherte Ereignisprefix lückenlos von 1 bis zur bestätigten letzten
Sequenz vorliegen. Gelöschte Ereignisse oder manipulierte Zeilen blockieren die
Schreibbarkeit; neue Antworten und Entwurfsschreibvorgänge bleiben gesperrt. Dies erklärt nicht jede Unterbrechung
als Abbruch. Bereits bestätigte Speicher- und Antwortbelege bleiben abgleichbar.
Es gibt keine ungeprüfte Wiederholung einer externen Aktion.

## Dienst- und Clientverträge

Alle folgenden Pfade liegen unter `/api/v2`. Sie verwenden die bestehende
Daemon-Authentisierung, Originprüfung und Pfadvalidierung. Das globale HTTP-Limit
bleibt 1.100.000 Bytes; die engere Antwortgrenze wird zusätzlich geprüft.
Command-Datentypen verweigern unbekannte Felder. Rechte, Owner oder RunSpec können
nicht über diese Bodies geliefert werden.

| Pfad | Verhalten |
| --- | --- |
| GET `/runs/{run}/workflow` | Fragen, Knotenstatus, Receipts und separate Resume-Fähigkeitsangaben |
| GET `/runs/{run}/workflow/inputs/{request}/draft` | `WorkflowInputDraftLookup` mit Head, Snapshot- und Intentmetadaten |
| PUT derselbe Entwurfspfad | `SaveWorkflowInputDraft`; bestätigter Saved-Beleg oder HTTP-409-Conflict-Beleg |
| GET `/runs/{run}/workflow/inputs/{request}/draft/snapshots/{snapshot}` | Ein geprüfter unveränderlicher Snapshot |
| POST `/runs/{run}/workflow/inputs/{request}/answer-intents` | `PrepareWorkflowInputAnswer`; eingefrorener Vorgang ohne Antwortwirkung |
| GET `/runs/{run}/workflow/input-answer-intents/{intent}` | Vollständiger gespeicherter Vorgang und aktueller Beleg |
| POST `/runs/{run}/workflow/input-answer-intents/{intent}/commit` | Bodyloser Commit derselben ID |

Der alte Einzelagentenpfad `/runs/{run}/input` nimmt keine Workflowantworten an.
Sein bisheriges Einzelagentenverhalten bleibt getrennt erhalten.

Rust ist die Quelle für die 161 generierten Verträge dieses qualifizierten Standes einschließlich
OpenAPI, TypeScript und Laufzeitvalidatoren. Der gemeinsame Client prüft Antwort-,
Snapshot- und Vorgangsbindungen und dekodiert den gespeicherten HTTP-409-Konflikt
gezielt. Die Oberfläche serialisiert Entwurfsspeicherungen, erhält unbestätigte
Originaltexte und Schlüssel und zeigt Konflikte, Quoten und ungeklärte Vorgänge.
Client-, Projekt-, Lauf- und Fragewechsel besitzen eigene Sessiongrenzen;
veraltete asynchrone Arbeit darf weder einen neuen Request auslösen noch aktuellen
Text ersetzen. Das gilt auch bei React StrictMode und gecachter Wiederöffnung.

## Quoten, Speicherung und Export

| Grenze | Wert |
| --- | --- |
| Fragetext | 4.000 UTF-8-Bytes |
| Antwortentwurf / eingefrorene Antwort | 32.000 UTF-8-Bytes |
| Snapshots pro Frage und Owner | 128 einschließlich Konfliktkopien |
| Antwortvorgänge pro Frage und Owner | 16 einschließlich terminaler Belege; höchstens ein Pending |
| Entwurfsheads pro Projekt | 512 |
| Snapshots pro Projekt | 4.096 |
| Serialisierte Snapshotdaten pro Projekt | 33.554.432 Bytes, also 32 MiB |
| Antwortvorgänge pro Projekt | 512 |

Quoten werden dauerhaft und atomar reserviert. Alte Snapshots und terminale
Vorgänge zählen weiter; dieser Schnitt enthält keine automatische Bereinigung.
Bei erschöpfter Quote bleibt der bestätigte Stand bestehen. Die Grenzen sind
Dienstgrenzen der persönlichen Vorschau, keine Enterprise-Budgetverwaltung.

Migration 19 ergänzt die Antwortentwurfs-, Snapshot-, Speicherquittungs-,
Intent- und Quotentabellen. Das interne Workflowformat ist 3; vorhandene Formate
1 und 2 bleiben lesbar. Das Ereignis-Wireformat bleibt Schema 7, da neue
Ereigniskinds die bestehende Form ergänzen. Auftragsarchive verwenden Exportformat
12 mit optionalem `human_inputs` im Workflowsnapshot und optionalen
`workflow_input_answer_records` im Auftragsarchiv. Alte Exporte ohne diese Felder
bleiben lesbar und behalten ihre ursprünglichen Hashregeln.

Ein autorisierter Export enthält auch Antworttexte, Entwürfe, Snapshots und
Vorgänge. Er liest und prüft diese in demselben konsistenten Datenbanksnapshot
wie Verlauf und Ereignisse. Import oder Archivierung erteilt keine aktiven Rechte
und aktiviert keine historischen Pending-Vorgänge. Ereignisse und Standardlogs
enthalten Metadaten und Hashes, keine menschlichen Antworttexte. Sichtbare
Antwortdaten bleiben innerhalb des bestehenden persönlichen Dienstzugriffs.

## Quellengebundene Abnahme und verbleibende Grenzen

Der Layout-Sammelbeleg (private development evidence; omitted from public sources) ist `passed`, SHA-256
`9b3ee83e1e4f40c345b63707bfac79e4a427f5688a506f04ac4fa3c21171fa26`.
Sieben Berichte binden 265 abgeglichene Eingaben. Die fünf Backendnachweise der
folgenden historischen Tabelle werden für genau ihren unveränderten Quellenstand
wiederverwendet; der neue Frontend-/Browserstand ist separat gebunden:

| Layoutnachweis | Bestandener Umfang | Bericht |
| --- | --- | --- |
| Frontend | 164 Produkttests, TypeScript, 161 Vertragsdefinitionen und Build; 73 Inputs/sechs Distdateien | Frontend (private development evidence; omitted from public sources) |
| Browser | 24 reale begrenzte Checks, 76 Inputs und dieselben sechs Distdateien; private Daemonkopie SHA 217d002f…; eigene Prozesse/Provider bestätigt gestoppt | Browser (private development evidence; omitted from public sources) |

Der Quellsnapshot (private development evidence; omitted from public sources) erhält 266 Inputs einschließlich Collector,
SHA-256 `22e7088fb3db3c3962d310509597eb8126d83c1b9a8a9ec842c6981acfada417`.
Alle Mitglieder des Quellarchivs (private development evidence; omitted from public sources) wurden geprüft; ZIP-SHA
`b13f19cad6fd1dd542890b5ab90efaef5c2728aa1de679b62e6870290ea869d6`.
Das ist kein Archiv des vollständigen Repositories, von Live-Daten oder Installern.
Die spätere Clock-/SQL-20-Arbeit ist ausdrücklich nicht durch diese Belege abgenommen.

### Historischer Rückfragen-Ausgangsnachweis

Der historische unveränderliche Input-Sammelbeleg (private development evidence; omitted from public sources)
steht auf `passed`. SHA-256:
`021ea3577fc356a65f4fcb9bafd0368f1ed8b44141568973adc0a413846418b7`.
Seine sieben Berichte binden insgesamt 263 erneut abgeglichene Eingaben; die
gemeinsamen Quellen stimmen über die Plattformnachweise hinweg überein.

| Nachweis | Bestandener Umfang | Bericht |
| --- | --- | --- |
| Windows-Rust | 364 Tests, Formatierung und Clippy; 184 Quellinputs, kein Stackoverride | Windows (private development evidence; omitted from public sources) |
| Ubuntu-Rust | 347 Tests, Formatierung und Clippy; 184 Quellinputs, kein Stackoverride | Ubuntu (private development evidence; omitted from public sources) |
| PostgreSQL | 11 tatsächliche getrennte Verträge bis Migration 19; privater PostgreSQL 16.15-Cluster bestätigt gestoppt | PostgreSQL (private development evidence; omitted from public sources) |
| Windows-Prozesse | Zehn echte Daemon-Prozesssuiten; 195 Inputs, eigene temporäre Daten und Loopbackports | Windows-Prozesse (private development evidence; omitted from public sources) |
| Ubuntu-Prozesse | Zehn echte Daemon-Prozesssuiten; 195 Inputs, eigene temporäre Daten und Loopbackports | Ubuntu-Prozesse (private development evidence; omitted from public sources) |
| Frontend | 145 Tests, TypeScript, Prüfung der 161 Verträge und Web-Build; 71 Quellen/sechs Distdateien | Frontend (private development evidence; omitted from public sources) |
| Browser | 20 tatsächliche Prüfschritte; 74 Quellen, identische sechs Distdateien und qualifizierter privater Daemon; eigener Daemon und Provider bestätigt gestoppt | Browser (private development evidence; omitted from public sources) |

Der historische 145-Test-Frontendbeleg umfasst auch die Korrektur der Anzeige nach verlorenem
Commit-Erfolg: Ein exakt gebundener terminaler GET-Beleg bestätigt den ursprünglichen
Vorgang. Pending, Lesefehler, falsche Identität oder Kontextwechsel bleiben unbestätigt
und erzeugen keinen Ersatz-POST. Der frühere 137-Test-Frontendbericht (private development evidence; omitted from public sources)
bleibt unverändert historisch; ebenso die älteren 144-Vertrags-/102-Test- und
18/7/11-Journal-/Snapshotnachweise.

Die historische 20-Schritt-Browserprobe bestätigt getrennte Fragen, genaue UTF-8-/LF-/CRLF-Texte,
verlorene Save-/Prepare-/Commit-Erfolgsantworten, immutable Konfliktkopien,
ausdrückliche CAS-Übernahme und sichtbare Ablehnung eines falschen Snapshothashs.
Tabbeendigung und eigener Daemonneustart erhalten Accepted-Receipts sowie unterbrochene
Entwürfe/Pending-Intents, ohne Antwort oder Modellwiederholung auszulösen. Die 390 px-
Prüfung beobachtet separate lesbare Felder, Tastaturfokus und keinen horizontalen
Bodyüberlauf. Sie ist keine vollständige Tastatur-, Zoom-, NVDA- oder WCAG-Abnahme.

Zwei zu starke Harness-Locatorannahmen bleiben im Browserbericht transparent:
eine sofort sichtbare Seitenleiste bei zunächst 390 px und eine automatische Öffnung
unterbrochener Frozen-Details. Die tatsächlichen Folgeprüfungen initialisieren bei
1440px und öffnen den gespeicherten Vorgang ausdrücklich über dessen Historie.
Diese Locator-Timeouts zählen weder als bestandene Schritte noch als nachgewiesene
Produktfehler. Die 20 tatsächlichen Prüfschritte bleiben gesondert ausgewiesen.

Im Windows-Prozesslog steht nach dem einzigen Passed-JSON zusätzlich genau ein
TCP-Readreset der eigenen synthetischen Loopback-Providerverbindung: Python-Stdlib
`handle_one_request`/`rfile.readline` meldet `ConnectionResetError WinError10054`,
bevor ein HTTP-Handler läuft. Der Collector erhält den exakten Diagnoseblock roh und
mit Log-SHA `a2048e609b97e33ab66a44ba11c6542f918d3b85cc4ee175f9c4b2095e851912`.
Exit0 und alle acht Input-HTTP-Szenarien bleiben nachgewiesen; zusätzliche unbekannte
Handlerfehler werden nicht akzeptiert. Die Zuordnung dieses erwartbaren Resets zu
einem bestimmten Hardkill ist mangels Zeit-/PIDkorrelation eine begründete Inferenz.
Das Ubuntu-Inputlog enthält keine solche Zusatzdiagnose.

Der frühere fehlgeschlagene Windows-Prozessbericht (private development evidence; omitted from public sources)
bleibt failed und unverändert: Er deckte den tatsächlich korrigierten
CancelRequested→Interrupted-Race auf. Der neue bestandene Sammelbeleg ersetzt ihn
nicht rückwirkend.

### Auftragsansicht: belegte Höhenkorrektur

Der historische 1440px-Screenshot (private development evidence; omitted from public sources)
zeigte etwa 90 px für Unterhaltung und Rückfragen. Der neue Schnitt macht den
angenommenen Startbeleg kompakt und die gesperrte Eingabe ausdrücklich aufklappbar.
Bei 1440×1000 sind **552,07 px eingeklappt und 429,74 px aufgeklappt** gemessen.
Pending 1440 (private development evidence; omitted from public sources)
und Interrupted 390 (private development evidence; omitted from public sources)
zeigen den tatsächlich gebundenen Stand.

Die erste reale Layoutprobe (private development evidence; omitted from public sources) bleibt unverändert `failed`:
389,74 px bei aufgeklappter Eingabe lagen unter dem Ziel von mindestens 400 px.
Die daraufhin von 360 auf 320 px reduzierte maximale Bedienbereichshöhe ist durch
frische vollständige Frontend- und Browsernachweise abgenommen. Frühere bestandene
Frontendvorversuche qualifizieren diese spätere CSS-Revision nicht.

Die kompakte Ansicht verlangt einen bestätigten, exakt an ursprünglichen Auftrag,
Projekt, angezeigten Lauf und unveränderliche Workflowversion gebundenen Startbeleg.
Effektive Budgets entsprechen den Schnittmengen aus Anfrage und Definition.
Loading, Fehler, Pending/ungeklärte Vorbereitung, Speichervorgänge, Konflikte oder
unbestätigte Entwurfänderungen lassen die Eingabe aufgeklappt. Der ursprüngliche
Starttext und Diagnosen bleiben ausdrücklich erreichbar. Ein Darstellungswechsel
führt weder Ack, neuen Start, Send noch Resume aus.

Die reale Probe prüft unveränderte Formular-/Textarea-DOM-Identität über
Pending→Accepted sowie erhaltenen nativen Budgetdetailszustand und Enter-Bedienung.
Eine verlorene Start-Prepare-ACK bleibt sichtbar; Reload erzeugt weder neuen Schlüssel
noch automatischen Commit. Die bisherigen Antwort-/Konflikt-/ACK-Prüfungen wurden
wiederholt. Tabclose und echter Neustart erhalten readonly Accepted-Receipts sowie
Interrupted-Entwürfe und FrozenPending ohne Providerreplay; es bleiben sechs
synthetische Modellaufrufe. Bei 390 px bleiben Tastaturfokus und Bodybreite 390 px.
Dies ist keine vollständige Zoom-, NVDA-, WCAG- oder native GUI-Abnahme.

Auf diesem historischen SQL-19-/Layoutstand war `input_resume_available` ausdrücklich **false**. Bei damaligen unterbrochenen
Läufen mit Fragen lautete der gesonderte Blockiergrund
`workflow_input_resume_unavailable`. Gesicherte Texte und ungeklärte Vorgänge
blieben erhalten; dieser damalige Schnitt enthielt keine Input-Resume-Aktion und kein automatisches
Weiterlaufen nach Daemonneustart. Die damalige Completion-only-Fortsetzung war
ein anderer, enger Pfad und qualifizierte keine Wiederaufnahme wartender Fragen.
Die neue ausdrückliche Human-Cursor-Fortsetzung ist separat im oben verlinkten aktuellen Beleg qualifiziert.

Externe Workflow-Datei-/MCP-/Prozesswerkzeuge, Hierarchie, allgemeiner belegter
Resume sowie die übrigen Skills-, Wissens-, Memory- und Zeitplanungsbereiche sind
hier nicht fertiggestellt. Enterprise-Teamrechte, Offline-Ausführung,
Compose-/Kubernetes-Betrieb, Last-, echte Modellhardware-, aktuelle native
Desktop-GUI-, Betriebssystemdialog-, NVDA- und vollständige Barrierefreiheitsabnahme
werden durch diesen Schnitt nicht freigegeben. M5 und der Gesamtneubau M0–M10
bleiben offen; produktive Umstellung und Release sind nicht freigegeben.
