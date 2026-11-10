# Private Ausführungsuhr für native Workflows

Stand: 9. Oktober 2026. Der private Clock-Schnitt ist im unten verlinkten
quellengebundenen Nachweis begrenzt geprüft. Er ersetzt keine vollständige
Produkt-/Plattformabnahme. Frühere Input-/Layoutbelege bleiben an ihren
archivierten Datenbank-19-Quellen gebunden und qualifizieren diesen veränderten
Kern oder einen Clock-Browserablauf nicht.

## Umfang und Formate

Die Uhrprojektion gilt für native Workflow-Läufe mit gespeicherter
Workflowbindung. Der gemeinsame Rust-Kern bleibt Bibliothek; Python ist nur
Sprache der Prozessfixtures und keine benötigte Agenten-/Crew-Laufzeit.
Einzelagenten, externe Programme, Dateibroker und MCP erhalten durch diesen
Schnitt keine pauschal neue Uhr- oder Wiederaufnahmeregel.

| Format | Implementierter Stand dieses Schnitts |
| --- | --- |
| Lokales/serverseitiges SQL-Schema | Migration 20 |
| Private Uhrprojektion | `workflow_execution_clocks`, Payloadformat 1 |
| Öffentlicher Kernvertrag | `SCHEMA_VERSION = 7`, unverändert |
| Auftragsexport | Format 12, unverändert |
| Menschliche Workflow-Wiederaufnahme | Weiterhin nicht freigeschaltet |

Die private Tabelle besitzt Lauf-ID, Projektionrevision, letzte bestätigte
Laufrevision, UTC-Hochwassermarke in Millisekunden und ein streng validiertes
Payload. Dieses bindet Projekt, Auftrag, ursprüngliche Erstellungszeit,
eingefrorene Runspec, Workflowbindung, Plan und initialen Workflowstand über
Hashes. Ursprünglicher Laufbeginn und bestätigter Erstellungszustand werden
zusätzlich gebunden. Die Quelle ist storelokal: Produktion verwendet UTC in
Millisekunden; interne Rusttests können eine private Quelle injizieren. Es gibt
keinen öffentlichen Zeitparameter, keine Testuhr aus einer Umgebungsvariable und
keinen Clock-Reset-Endpunkt.

## Ursprung und Migration

Ein Ursprung entsteht ausschließlich im tatsächlich neuen
`workflow_runs`-INSERT-Zweig derselben Starttransaktion. Der neue Parent ist
`queued`, Revision 1 und Ereignissequenz 2, ohne Lease, Beginn, Schritte,
Agenten oder Admissions. Der Workflow besitzt Revision 0 und den unveränderten
initialen Plan. Die abschließende Erstellungsprüfung verlangt die wirklichen
Ereigniszeilen `run_queued` und `workflow_initialized`, einschließlich ihrer
IDs, Sequenzen, Bindungen und Erstellungszeit. Ein frisches UTC-Sample nach den
bisherigen SQL-Waits bestätigt die gemeinsame Transaktion. Ein Fehlschlag
verwirft auch Auftrag-CAS, Lauf, Workflow, Uhr, Ereignisse und Startbeleg.

Ein Idempotenzreplay wird vor dem neuen INSERT gelesen. Es seedet keinen
zweiten Ursprung. Lesen, Import, Migration und konservative Wiederherstellung
sind ebenfalls keine Ursprungsquelle. Migration 19→20 enthält keinen Backfill
mit `now()`, einer vermuteten Startzeit oder einer rekonstruierten Restdauer.
Alte Läufe ohne Projektion bleiben ausdrücklich unverifiziert.

Die normalen Migrationprüfungen erhalten Sequenz und Prüfsummen und verweigern
unbekannte neuere Stände. Die SQL-20-Projektion ist nicht Bestandteil einer
neuen öffentlichen Exportautorität: Ein Format-12-Export erlaubt nicht, einen
fehlenden Ursprung beim Import zu erfinden. Datenbanksicherungen müssen den
konsistenten Datenbankstand einschließlich der privaten Projektion erhalten.
Dieser Schnitt ist noch keine vollständige Backup-/Restore-Abnahme.

## Transaktion und Ausführungsprüfung

Jede native Ausführungsänderung verwendet den gemeinsamen Run-CAS, auch ein
Heartbeat ohne neues Ereignis. Die Projektion folgt genau dieser bestätigten
Laufrevision. Kein unabhängiger Uhrwriter erhöht die Hochwassermarke ohne
Run-CAS. Eine frühe Stage oder ein Preflight ist kein Dispatchpermit.

Die Sperrreihenfolge bleibt: vorhandener Intent-/Drafthead, Registry, Run,
bereits benötigte Quoten/Dateiressourcen, schließlich Clock. Die Uhr wird zuletzt
gesperrt; danach werden keine Registry-, Run- oder Ressourcensperren nachgeholt.
Nach dem tatsächlichen Run-CAS prüft die Clockfinalisierung die gespeicherte
Vorherrevision, den wirklichen Nachherstand, Ursprung und Ereigniswirkung und
sampelt UTC erneut. Uhr-CAS und Laufwirkung werden gemeinsam bestätigt.

Die Transaktion merkt sich einen monotonen Sampleboden. Jedes weitere
Ausführungssample muss sowohl die persistierte Hochwassermarke als auch die
bereits in dieser Transaktion beobachteten gültigen Samples erreichen. Daher
wird auch `HWM+2 → HWM+1` verweigert, obwohl beide Werte über der bisher
persistierten Marke liegen. Nach letzten SQL-Waits folgt eine weitere frische
Prüfung unmittelbar vor Commit; ursprüngliche Lease-, Parent-, Knoten- und
Ahnenfristen bleiben zusätzlich erforderlich. Eine Rückstellung verwirft alle
betroffenen SQL-Wirkungen und Reservierungsänderungen.

Neue Antwortentwürfe und Prepare-Intents besitzen eigene Transaktionen. Unter
der bestehenden Runsperre prüfen sie vor Wirkung und vor Commit frisches UTC,
den Sampleboden und die ursprüngliche Fragenfrist. Sie verändern dabei weder
Laufrevision noch Hochwassermarke. Es gibt keine neue Ausnahme für das Sichern
von Text bei fehlender oder zurückgestellter Uhr. Bereits bestätigte Save-,
Prepare-, Accepted- und Startquittungen bleiben vor den Livegates als exakte
Lesequittungen erreichbar; veränderte Requests werden kein Replay.

## Modelle, Heartbeats und Werkzeugfreigaben

Vor dem ersten Credentialzugriff prüft die native Modellruntime die aktuelle
Uhrautorität. Nach Credential-/Kontextreads folgt erneut die Prüfung des exakt
eingefrorenen Turns. Die letzte Dispatchbeobachtung ist eine tatsächliche
Mutation unter Run-, Registry- und Clockfences. Sie prüft aktuelle Lease,
Running-Parent, genau denselben unbeantworteten Turn/Claim, aktiven Knoten,
Widerruf, Stop und ursprüngliche Fristen. Sie bestätigt keine Modellantwort
und erteilt keine zusätzliche Admission oder Rechte.

Providerstart erhält keinen dauerhaft gültigen Freibrief aus einem früheren
Snapshot. Zwischen lokaler DB-Prüfung und fremdem Provider bleibt allerdings
eine reale Prozess-/Netzwerkgrenze. Ein bereits zugelassener Request mit
unklarem Ausgang wird konservativ erhalten; der Schnitt verspricht keine
Exactly-once-Ausführung eines fremden Providers.

Coordinator-, Modellstreaming- und native Arbeitsverzeichnis-Snapshot-
Heartbeats verwenden `tokio::time::Instant` für ihren Takt. Der gemeinsame
Snapshotpfad nutzt weiterhin seinen 250-ms-Prüftakt und erneuert den Heartbeat
nach drei monoton gemessenen Sekunden; Stop-, Abbruch- und ursprüngliche
Budgetprüfungen bleiben erhalten. Ein UTC-Rücklauf darf den Heartbeattakt nicht
lange aussetzen. Ausführungsfristen werden weiterhin aus ursprünglichen
bestätigten absoluten Zeitpunkten abgeleitet; ein Neustart beginnt keine neue
relative Laufzeit.

Eine rein abschließbare terminale Workflowgrafik wird im gespeicherten
Abschlusszweig transaktional geprüft und beendet, bevor die Runtime weitere
Arbeitsverzeichnis- oder Profilzugriffe versucht. Dieser begrenzte Abschluss
startet weder Agenten noch Werkzeuge und erteilt keine neuen Rechte. Er ersetzt
keine menschliche Cursor-Wiederaufnahme.

Die öffentliche Workflowansicht macht offene Fragen bei typischem Clockdenial
nicht ausführbar und nennt den Grund. Sie liefert dann keine aktuell
bedienbaren Werkzeugfreigaben und keine Abschluss-Wiederaufnahme. Gespeicherte
Freigaben, Fragen, Antworten und Auditbelege bleiben erhalten. Der interne
Attentionindex ist eine abgeleitete Projektion ohne Ausführungsautorität;
seine Erstellungs-/CAS-Zwischenphase prüft keine noch unvollständige Clock.

Die Produktoberfläche zeigt für die exakten dienstseitigen Gründe
`workflow_clock_unverified` und `workflow_clock_rollback` einen übersetzten
Statushinweis in Deutsch und Englisch. Die Quelle ist der bestätigte
Parentfehler oder eine ausdrücklich dienstseitig geschlossene Frage mit diesem
Grund. Der Hinweis erklärt die gesperrten neuen Aktionen und erhält historische
Knoten und Ergebnisse; eine erkannte Rückstellung bleibt auch nach UTC-Aufholen
als Sperre verständlich. Die Anzeige löst keinen neuen Start, Retry, Send oder
Resume aus und entscheidet nicht selbst über Ausführungsautorität. Dieser enge
UI-Schnitt ist im neuen Frontendbericht statisch geprüft; frühere Layoutberichte
prüfen diese zusätzliche Anzeige nicht. Ein neuer Clock-Browser-/GUI-Durchlauf
gehört nicht zu diesem Nachweis.

## Sperren und begrenzter Abschluss

| Zustand | Fachliches Verhalten |
| --- | --- |
| Fehlende Projektion | `workflow_clock_unverified`; neue Ausführung und neue Antwortwrites werden verweigert |
| Frisches Sample unter gültiger Marke/Sampleboden | `workflow_clock_rollback`; keine neue Wirkung |
| Festgestellte fremde, korrupte oder widersprüchliche Projektion | Integritätsfehler `Forbidden`; kein stiller Ersatz durch einen Legacyursprung |
| Bereits bestätigter Clockblock | Bleibt eine Ausführungssperre; UTC-Aufholen aktiviert den alten Eintrag nicht automatisch |

Die Queue protokolliert eine typisierte Nichtaktivierung am weiterhin
angenommenen `queued`-/`waiting_for_executor`-Parent. Sie erzeugt dabei weder
Lease, Beginn noch einen Agentenfehler. Ein wiederholter Scanner erzeugt nicht
fortlaufend neue gleichartige Blockereignisse und setzt seine Suche nach
anderen zulässigen Aufträgen fort. Der ursprüngliche Accepted-Startbeleg bleibt
unverändert. Es gibt hier keine automatische Reparatur, Wiederaufnahme oder
allgemeine `queued → interrupted`-Ausnahme.

Validierte Cancellation, konservative Recovery und diese typisierte
Nichtaktivierung können begrenzt auch ohne verifizierte neue UTC-Ausführung
abschließen. Ihre Wirkungen werden ausdrücklich kontrolliert: keine neuen
Admissions, Antworten, Toolreceipts, Artefakte, Arbeitsverzeichnisclaims,
Umgebungen, Rechte, Leaseerneuerungen oder Provider-/Prozess-/Credential-IO.
Eine korrupte Projektion ist kein Anlass, einen Ursprung nachträglich zu
rekonstruieren. Unbekannte Modell-/Werkzeugwirkung bleibt als unbekannt markiert.

Reine Abschlussbelege verwenden eine monotone Belegzeit aus bestätigten
Lauf-/Domainzeiten und gegebenenfalls der vorhandenen Hochwassermarke; sie
spenden weder Restzeit noch Autorität. Eine reine Intent-Rejection erhält
`updated_at = max(created_at, bisheriges updated_at, bestätigtes Parent.updated_at,
nichtnegatives aktuelles UTC)`. Dadurch bleibt die Quittung auch nach
Rückstellung lesbar, ohne eine Antwort auszuführen oder die Clock zu erhöhen.
Ursprüngliche Budgets, Beginn, Charges und unbestätigte Reservierungen bleiben
erhalten; Zeitablauf oder Verbindungsverlust gibt keine unbelegte Kapazität frei.

## Quellengebundener Clock-Nachweis

Sammelbeleg (private development evidence; omitted from public sources), SHA-256 `c6d04795cd88c606517c50d95f61e9ff9c4770c60aed680c93ea4ceb8d16ee3f`; 6 Berichte/269 erneut abgeglichene Eingaben.

| Nachweis | Bestandener begrenzter Umfang |
| --- | --- |
| Windows (private development evidence; omitted from public sources) | 385 Rusttests, Format/Clippy; 189 Inputs, kein Stackoverride |
| Ubuntu (private development evidence; omitted from public sources) | 368 Rusttests, Format/Clippy; 189 Inputs, kein Stackoverride |
| PostgreSQL (private development evidence; omitted from public sources) | 13 getrennte tatsächliche Verträge bis SQL 20; privater Cluster bestätigt gestoppt |
| Windows-Prozesse (private development evidence; omitted from public sources) | 11 Suiten auf 201 Inputs; SHA-geprüfte private Binärkopie `a29e6fb951bd6a7148478ffd04c1396f3e6fc955f4ef386a8b66a9df1cb98182` |
| Ubuntu-Prozesse (private development evidence; omitted from public sources) | 11 Suiten auf 201 Inputs; SHA-geprüfte private Binärkopie `a7a0c999efba36e95444cdda3d211830f47d3f614f9139f0abbcfc7b41e5eccd` |
| Frontend (private development evidence; omitted from public sources) | 171 Produkttests, TypeScript, 161 Verträge und Build; 73 Inputs/6 Distdateien. DE/EN-Clockstatus statisch geprüft; kein neuer Clock-Browserlauf |

Quellsnapshot (private development evidence; omitted from public sources), SHA-256 `3734a0ddf971c994ab1cc5243d7a350b5bd9074ddd85d40ed3760edde37cfdfb`, 270 Eingaben; Quellarchiv (private development evidence; omitted from public sources), SHA-256 `3e3ae60d11426f9645ee21d318e244f086b3a9e9e1886200281fef7914cf5257`. Alle Archivmitglieder sind geprüft.

Der zusätzliche private Linux-Ausführungswrapper ist separat als Ausführungsinput in der Quellunion gebunden; die ursprünglichen gleichen Kern-/Prozessmanifeste bleiben unverändert. Sein reiner stdout-/stderr-Tee verändert weder Cargoargumente, Umgebung, Limits, Exitcodes noch Testharness. Originaler Qualifierbericht und rohe Livestreams sind im Sammelbeleg hashgebunden und gegen das vollständige Rustlog abgeglichen.

Die Clock-Prozessfixture verwendet je Plattform zwei eigene temporäre Datenbanken und sechs eigene Daemonstarts. Bei gestopptem Daemon wird die fehlende bzw. konsistente künftige Hochwassermarke eingerichtet. Die blockierte Arbeit erzeugt keinen Provider-GET/POST, keine Lease und keine Schritte, bleibt in Historie/Startquittung lesbar und erzeugt auch nach erneuten Scans/Neustart nur einen Blockbeleg. Anderer gültiger Workflow und Einzelagent schließen ab; eigene Daemons und Provider sind beendet.

Der frühere Layoutnachweis 19/7/12 mit 164 Frontendtests, 24 Browserchecks und 266 archivierten Inputs bleibt unverändert historisch. Seine 552,07/429,74-px-Messungen und DOM-/ACK-/Neustartproben sind keine neue SQL-20-Browserabnahme. Betriebssystem-Uhrrückstellung, native GUI/Dialoge, NVDA/WCAG, Enterprise/Last, vollständiges M5/M0–M10 und produktiver Release sind weiterhin nicht abgenommen.

## Qualifizierte Prüfpfade und weiter offene Produktgrenzen

Die folgenden Prüfquellen gehören zum begrenzten Nachweis oben; Syntaxprüfung
oder ein isolierter gerichteter Lauf allein wäre keine plattformübergreifende
Abnahme.

| Prüfquelle | Geprüfter implementierter Umfang des begrenzten Schnitts |
| --- | --- |
| [Private Clocktests](../crates/storage/src/workflow_clocks/tests.rs) | Ursprung, Erstellungs-COMMIT, Missing/Korruption/Rücklauf, monotone Beobachtung, reine Closure und Migrationgrenzen |
| [Antwort-/Produktclocktests](../crates/storage/src/workflow_input_drafts/clock_tests.rs) | Neun SQLitegruppen: Missing-/Rollback-/Korruptionsgates, unveränderte Replays, komplette SQL-/Quotenrollbacks, ursprüngliche Fragenfrist, reine monotone Rejection, Rücklauf oberhalb HWM und gesperrte Toolfreigaben |
| Dieselbe Antworttestdatei, ausdrücklich ausgeführter PostgreSQLtest | Vier tatsächliche Lockwait-Fälle: Save und Prepare jeweils hinter Run- und Quotarow-Sperren; späteres frisches Sample muss Wirkung samt Quoten verwerfen |
| [Prozessfixture](../tests/system/workflow_clocks.py) | Zwei getrennte temporäre SQLite-Szenarien mit Missing bzw. konsistentem Zukunfts-HWM, je drei eigene Daemonstarts: ursprüngliche Annahme ohne Worker, Worker-Neustart und zweiter Neustart; sechs eigene PIDs insgesamt |

Die Prozessfixture verändert ausschließlich ihren eigenen Datenbestand bei
gestopptem Daemon. Sie startet eine SHA-geprüfte private Binärkopie und einen
synthetischen Loopbackprovider. Sie prüft null Provider-GET/POST während der
isolierten Clockdenial-Phase, unveränderte Startquittung/Historie, genau einen
Blockbeleg, keine Lease/Schritte sowie anschließenden Abschluss eines gültigen
Workflows und eines Einzelagenten. Sie vergleicht Quellen/Binärdaten und meldet
`passed` erst nach Bereinigung ihrer eigenen Daemons und des Providers.
Das ist ein Zukunfts-Hochwassermarkenfixture, keine Änderung der Betriebssystemuhr.

Die quellengebundenen Windows-/Ubuntu-/PostgreSQL-/Prozess-/Frontendberichte
und der Clock-Sammelbeleg sind oben verknüpft. Native Dialoge/GUI, NVDA/WCAG, Last/Enterprise,
Backup-/Restore-Ziele, vollständige Wiederaufnahme, M5/M0–M10 und produktiver
Release sind durch diesen Schnitt nicht abgenommen.

Verwandte eingefrorene Produktbeschreibungen:
[Workflowprodukte](WORKFLOW-PRODUCT.md), [dauerhafte Vorgänge](WORKFLOW-JOURNAL.md),
[Input-/Layoutstand 19/7/12](WORKFLOW-INPUTS.md) und
[quellengebundene Prüfung](VERIFY.md).
