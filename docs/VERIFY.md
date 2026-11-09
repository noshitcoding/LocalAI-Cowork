# Lokale Prüfung — aktualisiert 2026-10-09

**Aktueller Paketstand:** [Installationslieferung vom 10. Oktober](INSTALLATION-DELIVERY-2026-10-10.md) sammelt die aktuellen nativen Paket-, Demo- und Serverbelege samt Grenzen.

**Aktueller Stand, 10. Oktober 2026, Singapur:** Der optimierte Windows-
Startfehler bei Autoritätsdateien ist behoben. Der aktuelle Release-Gate (private development evidence; omitted from public sources)
besteht 17 Daemontests, 33 Brokertests, Format und Clippy. Die zusätzliche
Dateinamensprüfung umfasst 64 ASCII-/Unicodepfade; der Broker prüft außerdem
Unicodepfade und den Schutz vorhandener Zieldateien. Die 1905 Quellen vor und
nach dem Gate sind identisch. Der Linux-Retest (private development evidence; omitted from public sources) besteht die
17 Daemontests und die neue Brokerprüfung auf demselben aktuellen Quellenstand.
Beide Systeme haben frisch daraus gebaute Release-Sidecars. Die eigenen
Testprozesse sind positiv beendet.

Die vollständigen früheren Workspace-Gates (private development evidence; omitted from public sources) mit
565 Windows-/548 Ubuntu-Tests und je 35 ausdrücklich ignorierten Tests
bleiben historische Nachweise: Zwei Coredateien wurden anschließend korrigiert.
Die unveränderten Frontendquellen bestehen weiterhin 202 Tests, TypeScript,
168 Vertragsprüfungen und Webbuild. Der neue optimierte Demo-Beleg (private development evidence; omitted from public sources)
bindet 1980 aktuelle Eingaben und 29 Belegdateien. Neun tatsächliche Browser-
Prüfgruppen bestehen, einschließlich Geldstatus, Verbindungswechsel, Neustart
und eines bestehenden lokalen Ollama-Modells `qwen3:4b`. Ein echter Lauf endet
mit der gespeicherten und angezeigten Antwort `Bereit`. Die drei eigenen normalen
Worker enden geordnet; alle 16 eigenen Jobprozesse sind positiv beendet.
Die native Desktop-GUI und ihre Pakete werden davon getrennt geprüft. Frühere
Paketbytes werden nicht als Nachweis des behobenen Starts verwendet.

Installationsvoraussetzungen und der noch offene vollständige M0–M10-/
Serverumfang stehen in [DEMO-AND-INSTALLATION.md](DEMO-AND-INSTALLATION.md)
und [COMPLETION-WORKPLAN.md](COMPLETION-WORKPLAN.md).

**Begrenzte Human-Resume-Abnahme (9. Oktober 2026):** Die ausdrückliche Wiederaufnahme belegter nativer Workflow-Rückfragen ist im neuen unveränderlichen Sammelbeleg (private development evidence; omitted from public sources) an ihre exakten Quellen gebunden. Windows/Ubuntu bestehen 387/370 Rusttests mit Format und Clippy, 14 tatsächliche PostgreSQL-Verträge, je zwölf Daemon-Prozesssuiten und 178 Frontendtests samt TypeScript, 163 Verträgen und Build. SQL 20/private Clock 1 bleiben erhalten; neue Continuations verwenden Progress 4, Ereignis 8 und Export 13. Resume und Accepted-Replay senden weder alte Modellanfragen noch gespeicherte Antworten. Antworten benötigen einen eigenen ausdrücklichen Commit. Der begrenzte Windows-Browserbeleg prüft Resume, Reload, Quittierung und separates Antwort-Commit; seine Fixture wurde extern unterbrochen, anschließend wurde ihre aktuelle Prozess-/Port-Abwesenheit geprüft. Kein historischer Browser-Reap oder Thread-Join wird daraus abgeleitet. Frühere Belege bleiben unverändert. Hierarchie, unbekannte Wirkungen, allgemeiner Resume, native GUI, NVDA/WCAG, Enterprise, vollständiges M5/M0–M10 und Release bleiben offen. Siehe [Ablauf und Grenzen](WORKFLOW-HUMAN-RESUME.md).

## Historischer quellengebundener Clock-Nachweis

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

**Historische Input-Layout-Abnahme (9. Oktober 2026, archivierter SQL-19-Stand):** Der unveränderliche Layout-Sammelbeleg (private development evidence; omitted from public sources) bindet sieben Berichte und 265 Eingaben: 164 Frontendtests auf 73 Quellen/sechs Distdateien sowie 24 tatsächliche Browserprüfschritte auf 76 Quellen. Bei 1440×1000 misst die Unterhaltung 552,07 px mit eingeklappter gesperrter Eingabe und 429,74 px aufgeklappt. Belegzugriff, identischer Formular-/Textarea-DOM, geöffnete Einstellungen, Tastaturfokus, verlorene Start-Prepare-ACK und Neustartpersistenz sind geprüft. Die fünf Backendberichte gelten für ihren eingefrorenen Datenbank-19/Ereignis-7/Export-12/161-Vertragsstand, nicht für die separat geprüfte private Clock-/SQL-20-Revision. Der 266-Input-Quellsnapshot (private development evidence; omitted from public sources) erhält exakt diese Revision. Der frühere Input-Sammelbeleg (private development evidence; omitted from public sources) bleibt unverändert historisch (145 Frontendtests/20 Browserchecks/263 Eingaben, Quellsnapshot mit 264 Inputs). Vollständiges M5/M0–M10, native GUI, NVDA/WCAG, Enterprise und Release bleiben nicht freigegeben.

**Historische Snapshot-Kopie-Abnahme (2026-10-09):** Historische Vorgangssnapshots lassen sich jetzt nach ausdrücklicher Prüfung als unabhängige Entwurfskopie wiederherstellen. Der separate Frontendbericht (private development evidence; omitted from public sources) besteht 102 Produktprüfungen auf 66 Inputs; der Browserbericht (private development evidence; omitted from public sources) besteht 14 begrenzte Prüfungen auf 69 Inputs und sechs Distdateien, eigener Daemon bestätigt gestoppt. Backend/Datenverträge waren bei dieser historischen Snapshotrevision unverändert; der eingefrorene Input-/Layoutstand ist im separaten Beleg oben abgenommen. Der Journal-Sammelbeleg 2a6e14a… bleibt erhalten; seine 86-/18-UI-/Distnachweise sind historisch gegenüber dieser Erweiterung. Browser-Dateiablage, native Dialoge, NVDA/WCAG AA, Enterprise und vollständiges M5 sind nicht abgenommen. Der historische Snapshot-Sammelbeleg (private development evidence; omitted from public sources) ist passed: sieben Berichte, 249 Eingaben, SHA256 bcbd57e6e93937b100447ce135d831c9fd20758b2fd76437ff0e4e3f09f5d425.

Die Ergebnisse gelten jeweils für den dokumentierten archivierten Quellstand. Es wurde kein
Commit oder Release erzeugt. V1-Installationen wurden nicht umgestellt.

## Quellengebundene Input-Layoutkorrektur — eingefrorener Stand 19/7/12

Der Layout-Sammelbeleg (private development evidence; omitted from public sources) ist `passed`, SHA-256
`9b3ee83e1e4f40c345b63707bfac79e4a427f5688a506f04ac4fa3c21171fa26`:
sieben Berichte, 265 abgeglichene Eingaben. Die fünf unveränderten Backendberichte
unten gelten weiterhin nur für ihre gebundene Revision; die neue Frontend-/Browser-
Qualifikation ersetzt die historischen UI-Nachweise nicht rückwirkend.

| Layoutnachweis | Bestandener Umfang |
| --- | --- |
| Frontend (private development evidence; omitted from public sources) | 164 Produkttests, TypeScript, 161 gemeinsame Verträge und Build; 73 Inputs/sechs Distdateien |
| Browser (private development evidence; omitted from public sources) | 24 echte Checks auf 76 Inputs, gleiche sechs Distdateien, private Daemonkopie mit SHA 217d002f…; eigener Daemon und Provider gestoppt |
| Unterhaltung bei 1440×1000 | 552,07 px mit eingeklappter gesperrter Eingabe, 429,74 px aufgeklappt |
| Darstellungszustand | Identischer Formular-/Textarea-DOM über Pending→Accepted, offenes Budgetdetail erhalten, Auf-/Zuklappen per Enter |
| ACK/Persistenz | Verlorene Start-Prepare-ACK bleibt sichtbar; Reload kein neuer Schlüssel/Autocommit. Antwort-/Konfliktfälle wiederholt, echte Tabbeendigung/Daemonneustart ohne neue Modellaufrufe oder Antworten |
| 390 px | Kein horizontaler Bodyüberlauf, sichtbarer Tastaturfokus; Pending- und Interrupted-Screenshots |

Der Quellsnapshot (private development evidence; omitted from public sources) bindet 266 Inputs einschließlich Collector.
SHA-256 `22e7088fb3db3c3962d310509597eb8126d83c1b9a8a9ec842c6981acfada417`;
Quellarchiv (private development evidence; omitted from public sources), SHA
`b13f19cad6fd1dd542890b5ab90efaef5c2728aa1de679b62e6870290ea869d6`.
Alle Archivmitglieder sind abgeglichen. Der Umfang ist keine vollständige
Repository-, Live-Daten-, Installer- oder Release-Sicherung.

Die 389,74-px-Vorprobe (private development evidence; omitted from public sources) bleibt unverändert `failed`. Erst die engere
320-px-Bedienregion und neue vollständige Frontend-/Browsernachweise erfüllen das
400-px-Ziel. Frühere bestandene Frontendberichte behalten ihre damaligen Quellen.
Die ältere 021e…-Inputqualifikation (145/20/263) und ihr
264-Input-Snapshot (private development evidence; omitted from public sources)
bleiben unverändert historisch. Die private Clock-/SQL-20-Abnahme besitzt den
separaten Nachweis oben; der Layoutbeleg gibt den veränderten Kern oder
Clock-Browserablauf nicht als geprüft aus. M5, native GUI, NVDA/WCAG, Enterprise und Release bleiben false.

## Historischer Workflow-Rückfragenstand — ursprünglicher Schnitt bestanden

Der unveränderliche Input-Sammelbeleg (private development evidence; omitted from public sources)
steht auf `passed`: sieben Berichte und 263 erneut abgeglichene Quellinputs.
SHA-256: `021ea3577fc356a65f4fcb9bafd0368f1ed8b44141568973adc0a413846418b7`.

| Historischer Ausgangsnachweis | Bestandener Umfang |
| --- | --- |
| Windows / Ubuntu | 364 / 347 Rusttests auf jeweils 184 Inputs, Formatierung und Clippy; kein Stackoverride |
| PostgreSQL | Elf tatsächliche getrennte Verträge bis Migration 19 auf privatem PostgreSQL 16.15-Cluster; Cluster bestätigt gestoppt |
| Daemonprozesse | Zehn Suiten je Plattform auf jeweils 195 Inputs; eigene temporäre Daten und Loopbackports |
| Frontend | 145 Tests, TypeScript, 161 Vertragsdefinitionen ohne Drift und Web-Build; 71 Quellen/sechs Distdateien |
| Browser | 20 tatsächliche Prüfschritte auf 74 Quellen, identische sechs Distdateien und qualifizierter privater Daemon; eigene Prozesse gestoppt |

Die einzelnen Berichtlinks, Quoten und Grenzen stehen in
[WORKFLOW-INPUTS.md](WORKFLOW-INPUTS.md). Die gemeinsame Quellunion umfasst 263 Inputs;
überlappende Quellen sind erneut geprüft. Datenbank 19, Ereignisschema 7, Export 12 und
161 Rust-generierte Vertragsdefinitionen sind der qualifizierte Stand.

Abgenommen sind je Agent gebundene Fragen bei Running-Parent/Lease, dauerhafte
SingleOwnerPreview-Antwortentwürfe, CAS und immutable Konfliktsnapshots, FrozenIntents
sowie ausdrücklich bodyloser ID-Commit. Graph, Agentenreceipt, Humanreceipt,
konkretes Ereignis und Accepted-Intent werden atomar bestätigt. Originaltext,
Schlüssel und Accepted-Historie bleiben nach verlorenem Acknowledgement erhalten.
Die Browserprobe bestätigt Tabbeendigung, eigenen Daemonneustart und erhaltene
Accepted-/Pending-Belege ohne automatische Antwort oder Modellwiederholung.
Auf diesem historischen Stand blieb `input_resume_available` ausdrücklich false.

Der historische 145-Test-Frontendnachweis umfasst die Lost-Commit-ACK-Anzeigekorrektur: Ein
gebundener terminaler GET-Beleg bestätigt den ursprünglichen Vorgang, während
Pending oder unpassende Belege unbestätigt bleiben. Der vorausgehende
137-Test-Bericht (private development evidence; omitted from public sources)
bleibt historisch an seine 71 Quellen/sechs Distdateien gebunden. Frühere
144-Vertrags-, 102-Test- und 18/7/11-Berichte behalten ebenfalls ihre ursprüngliche
Quelle; ihre Ergebnisse werden nicht auf den neuen Stand umgeschrieben.

Die zwei zu starken historischen Browser-Harness-Locatorannahmen bleiben transparent im
Browserbericht: sofort sichtbare Seitenleiste bei zunächst 390 px und automatische
Expansion des Interrupted-Frozen-Details. Die tatsächlichen Folgeprüfungen
initialisieren bei 1440 px und öffnen die Historie ausdrücklich. Die Timeouts
sind weder erfolgreiche Prüfschritte noch nachgewiesene Produktfehler.

Die Windows-Input-Prozesssuite besteht mit Exit0 und acht HTTP-Szenarien. Zusätzlich
steht genau ein TCP-Readreset in der eigenen synthetischen Providerverbindung vor
dem HTTP-Handler (`handle_one_request`/`rfile.readline`, WinError10054). Der
Collector bewahrt den exakten Rohblock und Log-SHA
`a2048e609b97e33ab66a44ba11c6542f918d3b85cc4ee175f9c4b2095e851912`.
Unbekannte Zusatz-/Handlerfehler werden nicht akzeptiert. Zuordnung zu einem
bestimmten Hardkill bleibt mangels Zeit-/PIDkorrelation eine begründete Inferenz.
Das Ubuntu-Inputlog ist ohne diese Zusatzdiagnose.

Der erste Windows-Prozessbericht (private development evidence; omitted from public sources)
bleibt failed: Nach neun grünen Suiten deckte er den echten
CancelRequested→Interrupted-Race auf. Der gespeicherte Intent blieb Pending und
Commit lieferte stale_lease. Die inzwischen quellengebunden geprüfte Korrektur
erhält bekannte Cancellation bei bestätigten Humanwaits oder konservativer
Modellunsicherheit; neue Antwort-/Draftwirkung prüft vollständige echte
Abbruchereignisse. Historische Fehlschläge werden nicht rückwirkend bestanden.

Die im historischen 1440px-Screenshot belegte etwa 90 px hohe Unterhaltung ist
im separaten Layoutschnitt oben korrigiert. Seine 24 tatsächlichen Checks und
552,07/429,74-px-Messwerte gelten für eigene Quellen und Distdateien; die damaligen
20 Checks werden weder erhöht noch auf den neuen Stand umgeschrieben.

Der historische Input-Sammelbeleg weist fullM5Acceptance=false, fullPlanComplete=false und
productionReleaseAllowed=false aus. Input-Resume war in diesem ursprünglichen Schnitt nicht
abgenommen; die neue begrenzte Human-Resume-Abnahme steht oben. Aktuelle native Desktop-GUI,
Browser-Dateiablage, NVDA/WCAG AA, Enterprise/Offline/Compose/Kubernetes/Last,
reale Modellhardware und produktive Veröffentlichung bleiben nicht freigegeben.

## Historische unabhängige Workflow-Snapshotkopie vor dem Inputstand

Der separate Sammelbeleg (private development evidence; omitted from public sources) ist passed:
sieben Berichte und 249 erneut geprüfte Eingaben. SHA256:
bcbd57e6e93937b100447ce135d831c9fd20758b2fd76437ff0e4e3f09f5d425.
Er verbindet fünf unveränderte Kern-/PostgreSQL-/Prozessberichte unten mit zwei neuen
Frontend-/Browserberichten. Er überschreibt den Journalcollector 2a6e14a… nicht.

| Prüfung | Tatsächlich bestanden | Bericht |
| --- | --- | --- |
| Snapshot-Kopie-Frontend | 102 Produktprüfungen, TypeScript, 144 Vertragsdefinitionen ohne Drift, Web-Build; 66 unveränderte Inputs/sechs Distdateien | Bericht (private development evidence; omitted from public sources) |
| Snapshot-Kopie-Browser | 14 begrenzte Prüfungen, 69 Inputs/sechs Distdateien, eigener Daemon PID42416 bestätigt gestoppt, Quelle/Dist/Daemon unverändert | Bericht (private development evidence; omitted from public sources) |

Frontendmanifest-SHA256: 0abcf96327bdac98433f0dd539853824465f50ca6a6c7ce8bbad070042701a73.
Browsermanifest-SHA256: 56aa39c82af2aec14219cb21cdbc50117dca917ae2f73661a0590b1a827c61e6.
Browserbericht-SHA256: 0dcc0b7f94c12aeadfd6357c8ff79013c1d0fe5138ab2a7d83e8e7cdb5aeb590.
Qualifizierter Daemon unverändert: 20526d0a6a3171039ebdc353730aa25879166d96c29a3c04ac33eb6caa8379d8.
Die zusätzlichen Browserinputs sind eigene Fixture-, Recorder- und Finalizerdateien.

Die echten Browserprüfungen bestätigen:

- Frische Vorgangs-/Snapshot-/Versionsprüfung liest den alten Stand gegen einen
  neueren Head und erzeugt weder Save noch Ack. Die tatsächliche StrictMode-Komponente
  bleibt nach Setup/Cleanup nutzbar.
- Die ausdrückliche Kopie verwendet neue ID/CAS0. Persistiertes verlorenes Save-Ack,
  ursprüngliches Accepted-Ack und identischer Retry erhalten den Editor, ursprüngliche
  Ziele/Modelle und genau eine bestätigte Kopierevision; ursprünglicher Head bleibt erhalten.
- Aktuelle Veröffentlichungsbasis lesen verändert nichts; erst ausdrückliche geprüfte
  Übernahme ändert Herkunft/Basis. Providerreferenzen bleiben gleich.
- Echte 32-Draft-Quote erhält Kopie und identischen Retry. Echte konkurrierende CAS-Anlage
  erhält Serverhead plus unveränderliche eingehende Kopie; ausdrückliches Keep Copy
  verwendet eine weitere unabhängige ID, ohne vorhandene Heads zu überschreiben.
- Unknown-Format, falscher SHA-/Herkunftsbezug und Pending bleiben readonly oder im
  vorhandenen Recoverypfad. Späte Antworten nach Projektwechsel erzeugen keinen Editor
  oder Save im neuen Kontext. Keine JavaScript-/Patternfehler oder unerwarteten APIfehler.
- Bei 390px liegen gespeicherte Untertitel vollständig innerhalb ihrer Schaltflächen;
  body overflow fehlt. Tastaturfokus bleibt im nativen HTML-dialog. CSS200%-Vergrößerung
  ist ausschließlich eine begrenzte Beobachtung, keine native Zoom-/NVDA-/WCAG-Abnahme.

Erwartete Konsolen-/Netzwerkbedingungen sind ausgewiesen: absichtlich verlorene
Save-Antwort (ERR_FAILED), tatsächliches budget_exceeded/CAS409 sowie favicon404 außerhalb
der API. Der ausdrücklich versuchte download.saveAs schloss den Testbrowserkontext;
Root beendete ihn nicht. Dieser Vorversuch und korrigierte Prüfselektoren bleiben im Bericht.
Die Kernprobe wurde danach vollständig mit neuer Aufzeichnung wiederholt.

Die Exportersatzprüfung erfasste 6687 tatsächliche Blobbytes samt unverändertem
CAS0-Request; SHA256 d0d96abb7c537ed5ee1470ce91ed1f4c0199e1ca5f81fdc0fd5a085957b3e033.
Nur die Anchor-Navigation wurde für diese Inhaltsassertion verhindert. Der gemeinsame
Browser-/Native-Exportpfad ist implementiert; browser_export_file_save_qualified bleibt
false. Keine neue erfolgreiche Browserdateiablage oder native Dialogabnahme behauptet.

Der Journal-Sammelbeleg bleibt unverändert. Dessen 86-/18-UI-/Distprüfungen und ältere
Desktopnachweise sind für diesen neuen UI-Schnitt historisch. Backend/Datenverträge
wurden nicht geändert. fullM5Acceptance, fullPlanComplete und productionReleaseAllowed
bleiben false. Details des Kopiervertrags stehen in [WORKFLOW-PRODUCT.md](WORKFLOW-PRODUCT.md);
dauerhafte Speicherkontrakte stehen in [WORKFLOW-JOURNAL.md](WORKFLOW-JOURNAL.md).

## Historischer Journal-Backend-/UI-Nachweis vor dem Inputstand

Der dauerhafte Anwendungsdienst speichert unvollständige Workflowformulare,
unveränderliche Save-/Konfliktsnapshots sowie eingefrorene Erstellen-/Publizieren-/
Sperren-/Startabsichten. Recovery lädt alle zum Projekt/Auftrag gehörenden
unbestätigten Pending-/Accepted-/Rejected-/Discarded-Belege vor neuen Schlüsseln.
Commit/Discard/Acknowledge verwenden nur deren ursprüngliche ID. Transiente
Transport-/Provider-/Speicherfehler erzeugen keinen Ersatzvorgang; Discard beendet
keinen bereits angenommenen Lauf. Der Produktumfang bleibt sequenziell/parallel
mit drei Datenbankartefaktwerkzeugen. Siehe [WORKFLOW-JOURNAL.md](WORKFLOW-JOURNAL.md)
und [WORKFLOW-PRODUCT.md](WORKFLOW-PRODUCT.md).

Der tatsächlich erzeugte und validierte
Journal-Sammelbeleg (private development evidence; omitted from public sources)
bleibt passed für den aufgezeichneten Stand: sieben Berichte, 245 damals abgeglichene Inputs und acht
gehashte historische Vorversuche. Collector-SHA256:
2a6e14a0e47fc69d25f6795c96761f37b94c2b23e64c37b0f642eb315d290d36.
Die 176 Kernhashes sind zwischen Windows, Ubuntu und PostgreSQL identisch,
die 186 Prozesshashes zwischen Windows und Ubuntu. Unterschiedliche JSON-
Manifestdateihashes sind kein Unterschied der geparsten Inputmaps.
fullM5Acceptance, fullPlanComplete und productionReleaseAllowed bleiben false.

| Prüfung | Tatsächlich bestanden | Bericht |
| --- | --- | --- |
| Historisches Journal-Frontend | 86 Produktprüfungen, TypeScript, Vertragsdrift und Web-Build; 63 Inputs | Bericht (private development evidence; omitted from public sources) |
| Windows-Kern | 348 Rust-Tests, Format und Clippy; 176 Inputs | Bericht (private development evidence; omitted from public sources) |
| Ubuntu-24.04/WSL2-Kern | 331 Rust-Tests, Format und Clippy; 176 Inputs | Bericht (private development evidence; omitted from public sources) |
| PostgreSQL 16.15 | Neun getrennte Speicherverträge bis Migration 18; eigener privater Cluster bestätigt gestoppt; 176 Inputs | Bericht (private development evidence; omitted from public sources) |
| Windows-Daemon | Build und neun echte Prozesssuiten; 186 Inputs | Bericht (private development evidence; omitted from public sources) |
| Ubuntu-Daemon | Build und neun echte Prozesssuiten; 186 Inputs | Bericht (private development evidence; omitted from public sources) |
| Historische Journal-Browserbedienung | 18 Prüfungen, 65 Inputs/sechs Distdateien; eigener Daemon gestoppt und Quellen/Dist/Binary unverändert | Bericht (private development evidence; omitted from public sources) |

Das Frontendmanifest besitzt SHA256
8552c370d093a1b8f77955145d4e7373936c992563e175454c025d2d9530a25a.
Der explizite Produktglob schließt private historische Desktopkopien unter .local aus.
Die früheren 140-/149-Testzähler enthielten 55 solche Tests; sie sind keine
aktuellen Produktzahlen. 144 generierte Verträge, Migration 18, Ereignisschema 7
und Exportformat 11 sind im Sammelbeleg gebunden. Der große Web-JavaScript-Chunk
bleibt ein Warnhinweis, kein bestandener Leistungsnachweis.

### Historische quellengebundene Journal-Browserprobe

Die frühere Browserprobe bindet ihre damalige Dist an den qualifizierten Daemon SHA256
20526d0a6a3171039ebdc353730aa25879166d96c29a3c04ac33eb6caa8379d8.
Ihr Manifest mit Frontendinputs und eigenem Fixture-/Prüfhelper besitzt SHA256
a4c45ee2709de3088ffa787e2c23d7406ca299c8b4be6094dc2d8017c59837a6.
Daneben liegen sources.json, dist.json, browser-proof.json, sechs Screenshots,
der bekannte synthetische unknown-draft.json-Export und stopped.json.

- Unvollständiger Entwurf mit leerem Modell/Ziel wird ohne Save-Schleife bestätigt
  und nach Reload exakt wieder geöffnet. Ein gehaltenes Save schützt vor Dirty-Escape;
  sauberes Schließen gibt den Tastaturfokus zurück.
- Unbekanntes Editorformat bleibt nur lesbar; der tatsächliche Export im privaten
  Testverzeichnis bewahrt das bekannte synthetische Dokument unverändert.
- Eine nach Persistierung verlorene Prepare-Antwort zeigt nach Reload denselben
  Pending-Publikationsvorgang und blockiert neue Schlüssel.
- Eine nach Annahme verlorene Publikationsantwort zeigt nach Reload denselben
  Accepted-Beleg ohne zweite Version. Nach separat erzeugtem Serverstand 3 öffnet
  der Beleg weiterhin seine originale Version 2.
- Echte parallele Headänderung führt zu CAS409 mit eingehendem Immutable-Snapshot.
  Die ausdrücklich gewählte unabhängige Editorfassung entspricht exakt dieser Kopie;
  die Serverfassung bleibt erhalten.
- Historische Version 1 trägt ihren alten Namen. Eine verlorene Startantwort öffnet
  nach Reload denselben angenommenen Lauf ohne doppelten Start.
- Zwei exakte rollenbezogene Agentenfreigaben sind bei laufendem Parent sichtbar.
  Eine Entscheidung erfolgt per Fokus/Enter. Zwei Artefakte sind in Inhalt, Größe,
  Herkunft und Hash geprüft; eine gemeinsame Assistant-Antwort wird gespeichert.
  Acknowledge/Reopen behält denselben completed-Lauf und die vier ursprünglichen
  synthetischen Modellaufrufe.
- Arbeitsdateien initialisieren, aktualisieren und öffnen erneut mit HTTP 200;
  historische Ergebnisse bleiben sichtbar. Die Budgetsummary zeigt 262144 und der
  create_artifact-Vertrag text/plain; charset=utf-8. read_artifact arguments.id wird
  im qualifizierten SSR-Test geprüft, nicht vom create-only Browserprovider ausgeführt.
- DE/EN, 390/1440 Pixel ohne horizontalen Bodyüberlauf und begrenzte
  Tastatur-/Fokusbedienung sind tatsächlich beobachtet. CSS 200 %-Vergrößerung bleibt
  eine Simulation; Hintergrundheading-Reflow und die enge Draftlisten-Unterzeile
  sind kein bestandener realer 200%-Zoom-/WCAG-Nachweis.

Konsole und Netzwerk wurden vor Browser-/Daemonende gesichert. Genau drei
ERR_FAILED-Meldungen gehören den absichtlichen verlorenen Antworten, der eine
HTTP 409 der erhaltenen Konfliktkopie. Unerwartete API-, JavaScript- oder
HTML-Patternfehler wurden nicht beobachtet. Der Browser wurde vor dem eigenen
Daemon geschlossen, PID 18276 beendet; ownDaemonStopped sowie die abschließende
Quellen-/Dist-/Binaryprüfung sind true. Keine bestehende Vorschau oder gestagte
Sidecardatei wurde ersetzt.

### Journalvorversuche und verbleibende Grenzen

Der erste Browserbericht
.local/workflow-journal-browser-2a8b331aa3804f568aa37441a573a523/result.json
bleibt failed: 106 erfolgreiche PUTs desselben uneditierten Formulars entstanden
durch verschiedene JSON-Objektschlüsselreihenfolge. Der eigene Prozess wurde
bestätigt gestoppt, Quellen/Dist/Binary blieben während jener Probe unverändert.
Kanonischer ordinaler Objektvergleich, Regression, neuer Produkt-Frontendlauf
und die tatsächliche neue Browserprobe beheben den Fehler. Der frühere be81-Bericht
bestand seine damaligen Unitprüfungen, qualifizierte diesen Browserfehler aber nicht.
Der e361-Frontendlauf sammelte zusätzlich private V1-Kandidattests und historische
Desktopkopien ein; er bleibt fehlgeschlagen. Alle acht im Collector gehashten
Vorversuche behalten Ergebnis, Fehlergrund und damaligen Quellenumfang.

Die früheren Workflow-Produkt-/Windows-/Linuxdesktopberichte sind nach den neuen
Journalquellen ausdrücklich historisch. Die gemeinsame Exportimplementierung nutzt
zwar die native Save-Bridge; interaktive OS-Dialoge, neue Desktop-Journal-GUI,
Secret Service, Installer, NVDA und vollständiges WCAG 2.2 AA sind nicht abgenommen.
Die frühere direkte Snapshot-Editorlücke ist durch die unabhängige Kopie oben geschlossen.
Quoten zählen
bestätigte/aufgegebene Historie weiter; automatische Bereinigung fehlt.
Zum damaligen Journal-/Snapshotstand fehlten noch Workflow-Rückfragen. Der darauf folgende
begrenzte Inputstand implementierte sie, ließ aber Input-Wiederaufnahme noch offen.
Die enge Human-Cursor-Wiederaufnahme ist nun separat oben qualifiziert; allgemeiner Resume bleibt offen. Noch nicht jeder Produktbefehl verwendet das Journal. Externe Werkzeuge,
Hierarchie, allgemeiner Resume, Skills/Wissen/Memory/Zeitplanung, Teams/Offline,
Compose/Kubernetes, reale Last, M0/M9 und vollständiges M5 bleiben offen.

## Historischer Workflow-Produktstand vor dem Journal

Die Angaben in diesem Abschnitt beschreiben den damaligen 55-Test-/Schema-17-Stand.
Sie sind keine Qualifikation der neueren Journalquellen oder aktuellen Desktop-GUI.
Der historische Sammelbeleg bleibt unverändert; aktuelle Nachweise stehen oben.

Die öffentliche projektbezogene Ablaufbibliothek, gemeinsame Preflight-/Startverträge,
autorisierter Workflowview, Editor/Versionsauswahl, agentenbezogene Freigaben und
Attention-Anbindung sind implementiert. Der Produktschnitt unterstützt sequenzielle
und parallele Modelle mit create/list/read Datenbankartefakten. Hierarchie,
Datei-/MCP-/Prozesswerkzeuge, Rückfragen, vollständiger Abbruch und allgemeiner
belegter Resume sind nicht freigeschaltet.

Der Sammelbeleg
workflow-product-2026-10-09.json (private development evidence; omitted from public sources)
ist erzeugt und validiert: bestandene aktuelle Berichte, 247 erneut abgeglichene
Dateien in der Quellenvereinigung, gesonderter vollständiger Workflow-Browserbeleg
und endgültige M0-Quellreferenz. fullM5Acceptance, fullPlanComplete und
productionReleaseAllowed bleiben false.

Die folgenden abgeschlossenen Berichte gehörten zum damaligen Produktschnitt.
Ihre Quellenmanifeste wurden damals gegen den damaligen Arbeitsbaum geprüft;
das ist keine Behauptung aktueller Hashgleichheit nach den Journaländerungen.
Die 165 Kern-Dateihashes sind zwischen Windows, Ubuntu und PostgreSQL identisch;
die 174 Prozess-Dateihashes sind zwischen Windows und Ubuntu identisch. Unterschiedliche
JSON-Manifestdateihashes allein sind kein inhaltlicher Quellenunterschied.

| Prüfung | Tatsächlich bestanden | Bericht |
| --- | --- | --- |
| Gemeinsames Frontend | 55 Tests, TypeScript, Vertragsdrift und Web-Build; 57 Quellen | `.local/workflow-product-client-a376947365d747ef8a4236fa686807d9/result.json` |
| Windows-Kern | 323 Rust-Tests, Format und Clippy; 165 Quellen | `.local/workflows-1627e06dd26f419e987dfb1b71e94550/result.json` |
| Ubuntu-24.04/WSL2-Kern | 306 Rust-Tests, Format und Clippy; 165 Quellen | `.local/workflows-0948df1588734572807ccc115934ffb9/result.json` |
| PostgreSQL 16.15 | Acht getrennte Speicherverträge bis Schema 17; Testcluster gestoppt | `.local/postgres-isolated-34678b992d874f6bb20f56b37edc26af/result.json` |
| Windows-Daemon | Build und acht echte Prozesssuiten; 174 Quellen | `.local/workflow-process-windows-32e4e4097050460889ba6c797eaa8522/result.json` |
| Ubuntu-Daemon | Build und acht echte Prozesssuiten; 174 Quellen | `.local/workflow-process-ubuntu-b88e1d42e0b147dc8c9c484634da950d/result.json` |
| Windows-Tauri | Sechs vorhandene native IPC-/Schlüsselablauftests, Format, Clippy und Build; 51 Quellen | `.local/desktop-workflow-windows-d0ce86ee272147708a00f1acdbaf5230/result.json` |
| Linux-Tauri/WebKit | Sechs native Tests, Format, Clippy, Build und tatsächlicher WebKit-Start unter Xvfb; Einzelagent übersteht Fensterschließen; 231 Quellen und sechs Distdateien | `.local/desktop-workflow-linux-c990a3d26ebc4630942652324b7ec059/result.json` |
| Windows-Workflow-Lebenszyklus | Privater Desktopbuild, zwei Rollen nach Desktopbeendigung, zwei exakte HTTP-Freigaben und zwei hashgeprüfte Artefakte; Reopen gleicher Daemon ohne weitere Modellaufrufe; 245 erfasste Dateien einschließlich sechs Distdateien | `.local/windows-workflow-lifetime-fb975b2adf9a4af3bdc5743bb06f9366/result.json` |

Das damalige Produkt-Frontendmanifest besitzt SHA256
`bb6add6e9bf50be51a07dd0a80da184768dfcf96cccbe21e6428b48e56f991be`.
Die Facade erzeugt 127 Definitionen. Speicherquellen besitzen Migration 17 für
den abgeleiteten Attention-Index; Ereignisschema 7 und Exportformat 11 bleiben
die verwendeten Vertragsversionen. Ein alter Schema-16-Bericht qualifiziert die
neue Migration nicht automatisch. Der Web-Build meldet weiterhin einen großen
JavaScript-Chunk; der Warnhinweis wird nicht als Testfehler ausgegeben.

Clienttests verwenden synthetische Antworten und tatsächliche Requestserialisierung;
Paneltests verwenden React-Rendering. Sie prüfen konkrete Versionswahl,
Preflightbindung, identische Wiederholung unbestätigter Publikation, Freigaben
bei laufendem Parent, separate Rollen, Fortsetzungssperren und die Reihenfolge
der Arbeitsdateiabfragen bei Initialisierung und gecachter Revalidierung.
Der native Desktop-Testumfang ist getrennt von diesen Tests. Linux verwendet
eine eigene Snapshotkopie, private DBus-/Xvfb-Sitzung und die aktuelle Dist.
Keine vorhandene Vorschau wurde neu gestartet, kein ursprünglicher gestagter
Sidecar ersetzt und keine WebKit-Sandbox abgeschaltet. Native Dialoge, Secret
Service, Installer und vollständige Desktop-Workflowbedienung sind dadurch
nicht abgenommen.

### Windows-Workflow nach Desktopbeendigung

Der getrennte Windows-Lebenszyklusbericht (private development evidence; omitted from public sources)
ist am damaligen Stand bestanden. Er prüfte 245 erfasste Dateien einschließlich
der sechs damaligen Distdateien; die Hashprüfung bleibt auf jenen Stand beschränkt.
Manifest-SHA256:
`c7ae01d69ca641b37b89f04560be3221b4e1c161d5a4913c1f71f3fd6569c629`.
Die private Buildkopie ergänzt ausschließlich eine leere `[workspace]`-Tabelle
in apps/desktop/src-tauri/Cargo.toml, um die eigenständige Desktop-Workspacegrenze
unter dem Repository zu erhalten. Runtime, Abhängigkeiten und Produktquellen
wurden dadurch nicht verändert.

Der damalige eigene native Debugdesktop startete den damaligen qualifizierten Daemon
`4fa5b945ef214abca41f1cd8995199179d26c60c270c87004f7d621bd792be63`.
Nach Desktopbeendigung läuft der angenommene Zweirollenworkflow weiter.
Zwei exakte Agentenfreigaben über die authentifizierte HTTP-API erlauben zwei
Artefakte; Inhalte, Herkunft und Hashes werden unabhängig geprüft.
Erneutes Öffnen verbindet sich mit derselben Daemon-PID; der abgeschlossene
Lauf bleibt completed und es entstehen keine weiteren Modellaufrufe.
Alle vier synthetischen Modellanfragen gehören zur ursprünglichen Ausführung.
Eigene Desktop-/Daemon-/WebView-PIDs sind bereinigt und der eigene Port ist frei.
Der ursprüngliche gestagte Sidecar und vorhandene Vorschauen blieben unverändert.

Die erste Probe scheiterte an einer Arrayform im Prüfhelfer; Diagnose und erster
Bericht bleiben erhalten, der korrigierte zweite Durchlauf ist maßgeblich.
Dieser Prozess-/API-Nachweis prüft keine GUI-Klicks oder tatsächliche
Workflow-Historienrenderung im Desktop. Native Dialoge, Installer, Signatur,
saubere Rechnerinstallation und reale Modelle bleiben unqualifiziert.

### Tatsächliche Browserbedienung und Dateiansicht

`.local/workflow-browser-dedc2b609ab448cdaa0b9f2ae24b2758/result.json`
besteht einen tatsächlichen UI-Durchlauf auf dem damaligen 53-Test-Stand:
Veröffentlichung einer umbenannten Version 2, erhaltener Name der immutable Version 1
in Bibliothek und Composer, ausdrückliche Einzelagent-/Ablaufwahl, Start von Version 1,
zwei exakte rollenbezogene Freigaben bei laufendem Parent, Eingang/Aktivität und
Abschluss mit zwei Artefakten und genau einer gemeinsamen Antwort. Die Vorschau zeigt
262144 Gesamttoken und das Antwortlimit; create_artifact zeigt den tatsächlichen
festen Typ `text/plain; charset=utf-8`. Rollenpattern sind mit dem HTML-v-Modus
kompatibel. Dirty-Escape-Schutz, sauberes Schließen mit Fokusrückkehr und Start per
Tab/Enter wurden beobachtet. Bei 1440 und 390 CSS-Pixeln blieb die Oberfläche ohne
horizontalen Bodyüberlauf. Acht Screenshots und Console-/Netzwerkbelege liegen daneben.

Dieser Bericht ist ausdrücklich historisch gegenüber der letzten Dateiansichtskorrektur.
Eine spätere gecachte Wiederöffnung verursachte einen sicheren Brokerkonflikt:
HTTP 409 `workspace_busy`, Korrelation
`88b31b6d-6647-47f3-8dac-c504882cfec5`, ungefähr
2026-10-08T17:49:19Z bei bereits abgeschlossenem Workflow.
Arbeitsverzeichnis-Revalidierung und Listing überlappten die exklusive
Materialisierungssperre. Der Broker wurde nicht abgeschwächt. Die Oberfläche wartet
nun auf bestätigte und nicht mehr laufende Workspaceabfrage, bevor Listing/Vorschau
starten; Refresh wartet dieselbe Reihenfolge ab. workspace_busy wird als temporärer
Dateistatus gezeigt, echte andere Fehler bleiben Alerts. Es gibt keinen automatischen
Mutation-Retry.

`.local/workflow-browser-982b84caf60b4f4eb17dd0af9df094bf/result.json`
besteht den abschließenden gezielten Browserrecheck auf dem unveränderten
55-Test-/57-Quellenstand: Erstöffnung, drei gecachte Wiederöffnungen und vier
ausdrückliche Refreshs lieferten Workspace und danach Listing jeweils mit HTTP 200,
ohne Workspacekonflikt oder generischen Dateifehler. 1440-/390-Pixel-Dateiansichten,
zwei Screenshots sowie Console/Netzwerk vor dem Stop sind dokumentiert.
Seit dem vollständigen Workflowdurchlauf änderten sich ausschließlich
WorkingFiles.tsx, dessen Tests und i18n.ts; Workflowlogik blieb unverändert.
Beide eigenen Browserdaemons sind bestätigt gestoppt; Quellen, Dist und Binärdatei
blieben während der jeweiligen Probe stabil.

Die Browserprobe verwendet einen synthetischen Provider. Sie belegt keine reale
Modellqualität oder Enterprise-Last. read_artifact mit `arguments.id` wird vom
qualifizierten SSR-Test geprüft, nicht vom create-only Browserprovider.
Ein favicon.ico-404 im letzten Recheck blockierte keinen Produktablauf;
JavaScript-/Patternfehler wurden dort nicht beobachtet. Tatsächlicher 200-%-Browserzoom
konnte nicht nachgewiesen werden: Ctrl+Equal veränderte in dieser Automationsumgebung
weder Viewport noch DPR. NVDA, vollständige Tastatur-/Dialogabnahme, native
Betriebssystemdialoge und WCAG 2.2 AA bleiben offen.

### Historische Produktvorläufe

Die Frontendberichte 70df (43 Tests/55 Quellen), c0f6 (47/56) und a3c4 (53/57)
bleiben historische Nachweise. Tatsächliche Browserproben führten zu gemeinsamen
Budgetdefaults, gültigem Rollenpattern, immutable Versionsnamen, korrektem
Artefaktanzeigevertrag und schließlich serialisierter Arbeitsdateiabfrage.
Die früheren Windowsdesktopberichte 2464 und 7c325 behalten ihren jeweiligen
alten UI-/Distumfang und ersetzen den damaligen d0ce-Bericht nicht.

Windows 13ce und Ubuntu af295 scheiterten zunächst in neuen Race-Fixtures:
insert_provider ersetzt ein vorhandenes Profil nicht; widersprüchliche gespeicherte
und konfigurierte Profile wurden vom Produkt vor der erwarteten Providerprobe
abgewiesen. Die Fixture wurde mit neuer expliziter Profilidentität und darauf
gepinnter Workflowversion korrigiert. Auch Windows e9cc, Ubuntu 9ff und PostgreSQL
2b03 sind fehlgeschlagene historische Vorläufe; sie werden nicht als bestanden
geführt. Maßgeblich sind die oben genannten vollständigen Wiederholungen.

Ubuntu 7d703 brach zuvor wegen fehlender Offline-Cachedatei
`toml_edit 0.25.15+spec-1.1.0` ab. Gesperrte Crate-Dateien wurden anhand
Cargo.lock-SHA256 aus dem Windowscache übernommen; keine Paketversion wurde
geändert. Auch dieser Cachevorlauf ist kein bestandener Linuxnachweis.
Der erste Browserbericht 3715 wurde wegen der anfänglichen Workspace-Race als
fehlgeschlagen archiviert und nicht überschrieben.

Am damaligen Produktstand lagen Workfloweditorentwürfe sowie eingefrorene
unbestätigte Publikations-/Startrequests noch im Arbeitsspeicher. Migration 18
und der historische Journalnachweis oben schließen diese konkrete Lücke. Dialogschutz und beforeunload belegen keine
Wiederherstellung nach Fenster-/Prozessneustart. Diese Prüfungen schließen M5,
Enterprise-Betrieb und vollständige UI-Barrierefreiheit nicht ab.
Verträge und Betriebsgrenzen: [WORKFLOW-PRODUCT.md](WORKFLOW-PRODUCT.md).

### Aktuelle M0-Quellintegrität

`.reference/v1-0.3.12-source-build-inputs-2026-10-09-final-unit-fixtures/independent-verification.json`
prüft 1090 Dateien, 106 versionierte Quellassets und 46 Pflichtinputs einschließlich
der ausdrücklich synthetischen engineStore.test.ts bytegenau. ZIP-Größen, Hashes,
Dateimodi und CRC sind unabhängig geprüft; 14 frühere Referenzartefakte blieben
unverändert. Archiv-SHA256:
`d4c4f69a61392cdc75f73e9da036ee1aad198eacd208d9af10ac82bdf6d7efc4`.
Dies ist Quellintegrität, keine V1-Build-/Testabnahme. m0_complete und
full_test_source_reproducible bleiben false; Runtimebinärdateien, echte Daten,
vollständige Testzuordnung und Migration sind damit nicht qualifiziert.

## Historischer privater Workflow-Artefaktbrokerstand

Der folgende Bericht beschreibt die vorangehende private Artefaktbrokerrevision,
qualifiziert am 9. Oktober unter einem am 8. Oktober begonnenen Belegnamen.
Er qualifiziert keine neuen öffentlichen Workflow-Endpunkte, keinen Attention-Index,
keine Schema-17-Migration und keine neue Editor-/Composer-Oberfläche.

Der frühere private Workflow-Artefaktbroker bestand 301 Windows- und 284 Ubuntu-Rust-
Workspace-Tests, Formatierung und Clippy. Enthalten sind 26 Domänen-, 13 Speicher-/
Prozess- und 47 Modell-/Koordinator-/Worker-Szenarien pro Plattform, darunter 14 neue
Artefaktfälle. Windows, Ubuntu und PostgreSQL prüften identische Hashes von 155
Quelldateien gegen den damaligen Arbeitsbaum.

Die Datenbankwerkzeuge create_artifact, list_artifacts und read_artifact verwenden
die gemeinsame Policy mit eingefrorenen Schrittrechten. Freigaben binden Lauf,
Agent/Modellturn, normalisierte Argumente, Ziel und Policyrevision. Werkzeugbeleg,
Reducer-Entscheidung, Artefakt, Suchindex, Schritt und Ereignisse werden in einer
Transaktion gespeichert. Bestätigter Replay erzeugt keine weitere Wirkung oder
Schrittbelastung. Rohe Provider-IDs bleiben im Modellkontext erhalten und sind
intern nach Lauf, Agent und Turn getrennt.

Die Prüfungen decken parallele Agenten mit identischer Provider-ID, spätere Turns,
Zulassen/Verweigern, alle vier Zugriffsmodi, Folgeanfragen mit Werkzeugantworten,
Widerruf während und nach Freigabe sowie Rollback bei fehlgeschlagenen Ereignissen
und Artefakt-Inserts ab. Reads umfassen den autorisierten Auftrag und verifizierte
geerbte Branch-Snapshots. Fremde oder nachträglich umgebundene Artefakte werden
auch bei neu berechnetem Inhaltshash abgewiesen.

Abgelaufene Agentenfreigaben beenden ihren Schritt; unabhängig bereits laufende
Antworten bleiben bestätigt. Stop verhindert weitere Werkzeugfreigaben und erzeugt
einen bekannten Schrittfehler. Die Eltern-Lease bleibt während einzelner Freigaben
erhalten. Schritt-/Ahnenfristen und Stop-Regeln werden vor neuer oder bestehender
Freigabe und nach dem Datenbanklock geprüft; Lease und Frist zusätzlich vor Commit.
Alle früheren Queue-/Recovery-Szenarien gehören zur vollständigen Regression.
Unklare Modellanfragen bleiben reserviert und werden nicht automatisch wiederholt;
reine Abschlusswiederaufnahme verwendet bestätigte Antworten ohne Provideraufruf.

Acht Verträge bestehen auf getrennten neuen PostgreSQL-16.15-Datenbanken bis Schema
16. Zwei Prüfungen beobachten den blockierten Start- beziehungsweise Freigabevorgang
in ihrer eigenen Datenbank und halten den Lock jeweils 15,2 Sekunden. Die gültige
15-Sekunden-Lease läuft dabei ab. Ein weiterer Aufnahmetest hält den Lock über eine
gezielt verkürzte bestehende 400-ms-Lease. Änderungen einschließlich Ereignissen
werden zurückgerollt. Die temporäre unprivilegierte Loopback-Instanz ist gestoppt;
vorhandene Systemdatenbanken und Docker wurden nicht verwendet.

Alle sieben damaligen Daemon-Prozessprüfer bestanden pro Plattform mit identischen
163 Quellhashes. Windows-Tauri bestand sechs Tests, Format/Clippy und Build auf 51
damals erneut geprüften Quellen. Der ältere Frontendbericht umfasste 38 Quellen,
29 Tests und TypeScript/Web-Build; seine zu diesem Zeitpunkt unveränderten
Clienttests wurden für den Artefaktstand nicht wiederholt. Die damals 102
generierten Verträge und ihr Generator wurden gegen den früheren Driftprüfstand
abgeglichen. Diese Angaben beschreiben ausschließlich den historischen Stand. Export bleibt Format 11. Das private
Workflow-Journal verwendet Format 2 und liest alte Format-1-Stände weiterhin.
Vorschauen und gestagte Sidecars wurden nicht verändert. Linux-Desktop-, native
Dialog- und UI-Berichte bleiben historische Belege.

Erste gezielte Prüfungen fanden eine zu lange interne Werkzeug-ID und eine
Kapazitätsentscheidung auf einer veralteten Revision bei parallelem Modellabschluss.
Die ID ist jetzt ein vollständiger kanonischer SHA-256 im bestehenden 64-Zeichen-
Limit; reine Zulassungen prüfen nach Revisionsänderung erneut. Die unabhängige
Prüfung ergänzte Lease-/Commitfences, bekannte Budgetfehler, Widerruf sowie
Frist-/Stop-Regeln während wartender Freigaben.

Ein Linux-Folgelauf fand einen zeitabhängigen Parallelitätstest mit nur 150 ms
Antwortpause. Drei Parallelitätsbelege verwenden jetzt einen begrenzten Rendezvous
der echten A/B-HTTP-Anfragen: Ein serieller Ablauf scheitert verbindlich. Ein
Windows-Folgelauf überschritt die 12-Sekunden-Fixturegrenze des ConfirmAll-Ablaufs;
einzeln bestand dieser in 9,36 Sekunden. Die funktionale Testgrenze beträgt jetzt
30 Sekunden unter dem unveränderten 60-Sekunden-Laufbudget. Inhaltfreie Zustands-
diagnosen, neun exakte Freigaben und zwölf HTTP-Aufrufe bleiben verbindlich. Die
Produktionssteuerung blieb bei diesen Testkorrekturen unverändert. Alle endgültigen
Prüfungen liefen auf den korrigierten Quellen; Vorläufe bleiben im Gesamtbeleg.

Synthetische Antworten verwenden tatsächliche HTTP/SSE-Verbindungen und eigene hart
beendete Kindprozesse. Datenbank-Artefakte sind geprüft; externe Workflow-Datei-/
MCP-/Prozessausführung, Rückfragen, vollständiger Abbruch/Abgleich, Produkt-Preflight,
Workflow-API/UI, hierarchische Modellsteuerung und strukturierte Ausgabe sind offen.
Dies ist keine reale LLM-, Sandbox-, Stromausfall-, Enterprise- oder vollständige
Produktabnahme. Workerplätze und Graphparallelität sind keine globalen Provider-
oder Organisationsquoten. M5 und M0–M10 sind nicht abgeschlossen.

Historischer Gesamtbeleg: `.evidence/workflow-artifact-tools-2026-10-08.json`;
Verhalten und Grenzen: [WORKFLOWS.md](WORKFLOWS.md).
Windows: `.local/workflows-bf29af8a503443c4be05a567846a2d30/result.json`;
Ubuntu: `.local/workflows-7e399b9a11c3415396f5caaa981bfd12/result.json`;
PostgreSQL: `.local/postgres-isolated-bc2f29157c304404b21d394571044b42/result.json`.

## Historischer Workflow-Warteschlangenstand


Die vorangehende Queue-Revision bestand 283 Windows- und 266 Ubuntu-Tests samt

Format/Clippy auf 153 gleichen Quellhashes, Domain24/Storage11/Runtime33. Eine alte

Dateibesitz-Fixture wurde dort auf den verbindlichen Start mit echter Lease

angepasst; die Produktionsprüfung blieb erhalten. Ein Windows-Vorläufer kollidierte

beim Ersetzen der Daemon-Binärdatei mit dem eigenen Prozessprüfer; die abschließende

Prüfung wurde separat ausgeführt. Der unveränderte historische Gesamtbeleg ist

`.evidence/workflow-queue-2026-10-08.json`; er beschreibt noch keinen Artefaktbroker.


## Historischer Workflow-Koordinatorstand

Der frühere private Koordinator bestand 274 Windows- und 257 Ubuntu-Tests,
Formatierung/Clippy, acht PostgreSQL-Verträge und sieben Daemon-Prozessprüfer je
Plattform auf 153 gleichen Quellhashes. Die regulären Queue-Worker waren noch nicht
angeschlossen. Unveränderter Nachweis: `.evidence/workflow-coordinator-2026-10-08.json`.

## Historischer Workflow-Modellstand

Der vorherige private Modelladapter bestand 262 Windows- und 245 Ubuntu-Tests,
Formatierung/Clippy, acht PostgreSQL-Verträge und sieben Daemon-Prozessprüfer je
Plattform mit identischen 151 Quellhashes. Er enthielt noch keinen automatischen
Koordinator oder Gesamtabschluss. Seine unveränderten Berichte bleiben erhalten:
`.evidence/workflow-models-2026-10-08.json`.

## Historischer Workflow-Speicherstand

Der vorherige native Workflow-Speicherstand bestand 249 Windows- und 232
Ubuntu-Rust-Workspace-Tests, jeweils mit Formatierung und Clippy. Darin enthalten
sind 24 Domänenszenarien und zehn neue Speicher-/Prozessprüfungen. Beide Plattformen
und PostgreSQL prüfen dieselben 147 Quelldateien; alle endgültigen Hashes stimmen
zum damaligen Prüfabschluss mit den Testmanifesten überein.

Ein unveränderliches Workflow-Register und Checkpoints sind über Migration 16 mit
dem gemeinsamen Auftragsstart verbunden. Checkpoint, Modellbudget, Worker-Lease,
Laufrevision und Ereignisse werden zusammen gespeichert. Konkurrenz, gescheiterte
Transaktionen, Rechteentzug, Manipulation und tatsächliches Kindprozessende sind
geprüft. Der Prozessnachweis verwendet eine synthetische Wirkung im Testordner;
er qualifiziert keine reale Workflow-Werkzeugausführung, Sandbox oder Stromausfall.

Acht Speicherverträge bestehen auf getrennten neuen PostgreSQL-16.15-Datenbanken.
Die eigens initialisierte, unprivilegierte Loopback-Testinstanz ist anschließend
gestoppt. Der vorhandene Ubuntu-Systemcluster und Docker wurden dafür nicht genutzt.
Der Workflow-Vertrag umfasst konkurrierende Publikationen/Zulassungen und eine
Datenbanksperre über die Worker-Frist hinaus: keine neue Dispatch-Berechtigung
und keine freigegebene Reservierung nach Fristablauf.

Die sieben Daemon-Prozessprüfer bestehen auf Windows und Ubuntu. 29 Clienttests,
TypeScript/Web-Build und die Driftprüfung aller 102 erzeugten Verträge bestehen.
Die Windows-Tauri-Hülle besteht sechs Tests, Formatierung, Clippy und Build.
Vorhandene Sidecars und Vorschauen bleiben unverändert. Es gibt keine aktuelle
interaktive Dialog-, Linux-WebKit-, Workflow-UI-, Enterprise- oder Installer-Abnahme.
In diesem älteren Stand blieben parallele Produkt-Runtime, dauerhafte Agentenkontexte, API/UI, exakte Workflow-
Freigaben/Abbruch und belegte Wiederaufnahme bleiben offen; M5 ist nicht abgeschlossen.

Historische Gesamtbelege dieses Quellstands: `.evidence/workflow-storage-2026-10-08.json` und
[WORKFLOWS.md](WORKFLOWS.md).
Windows: `.local/workflows-0dc85b7a64cb478d90ad93c752e45992/result.json`;
Ubuntu: `.local/workflows-2711fe80274047a49251434a59095540/result.json`;
PostgreSQL: `.local/postgres-isolated-0701a526499f46419f7fec1eb4377dda/result.json`.
Die ersten Gesamtläufe enthielten noch drei Exportformat-9-Erwartungen; diese wurden
auf das neue Format 10 korrigiert. Der erste Zwei-Verbindungs-Test zeigte zusätzlich
einen unbehandelten SQLite-Sperrfehler im neuen Prüfschritt. Sichere reine
Transaktionswiederholung und ein zusätzlicher Lease-Commit-Fence sind implementiert;
die endgültigen Läufe bestehen. Frühere Fehlerberichte bleiben erhalten.

## Historischer Workflow-Compiler-/Broker-Stand

Der neue native Workflow-Compiler/Planer und die Broker-Sperrkorrektur bestehen
24 Workflow-Szenarien und die vollständigen Rust-Workspaces: 239 Tests unter
Windows, 222 unter Ubuntu 24.04/WSL2; Formatierung und Clippy bestanden.
Beide Prüfungen enthalten dieselben 141 Quelldateien mit identischen Datei-Hashes;
der endgültige Stand ist erneut gegen beide Manifeste geprüft. Sieben Windows-
Daemon-Prozessprüfer bestehen auf diesem Stand ebenfalls.

Windows: `.local/workflows-90034e26b4d949f6b68b907752a6902c/result.json`;
Ubuntu: `.local/workflows-97c12b818a3243389b90f3acb7a5432c/result.json`.
Gesamtbeleg: `.evidence/workflow-foundation-2026-10-08.json`.

Der erste Ubuntu-Lauf scheiterte an unerwartetem `WorkspaceBusy` im Ordnerbroker.
Zwei neue Unix-Deskriptorprüfungen und 40 Wiederholungen aller sechs parallelen
Registry-Szenarien bestehen nach expliziter Sperrfreigabe, ebenso der nachfolgende
Gesamtlauf. Historische Fehler sind damit nicht alle abschließend erklärt;
siehe [BROKER-LOCKS.md](BROKER-LOCKS.md). Docker konnte wegen einer nicht zugreifbaren
Inference-Socket-Datei nicht starten; der aktuelle Linux-Nachweis stammt aus der
vorhandenen unprivilegierten Ubuntu-Umgebung und qualifiziert keine Container-Sandbox.

Dieser frühere Workflow-Stand prüfte serialisierte Checkpoints; transaktionale
Speicherung und tatsächliche Prozessdauerhaftigkeit sind im aktuellen Nachweis
oben ergänzt. Parallele Produkt-Runtime, API/UI und sichere belegte Wiederaufnahme
bleiben offen; siehe [WORKFLOWS.md](WORKFLOWS.md).

Der vorausgehende MCP-Produktstand besteht 215 reguläre Rust-Tests unter Windows und
196 im isolierten Linux-Testcontainer, dazu Formatierung und Clippy. Sechs neue
Runtime- und zwei neue HTTP-Tests je Plattform prüfen den Weg vom ausgewählten
Werkzeug über Modellschleife und Freigabe bis zum Remote-Beleg. Sieben PostgreSQL-
Verträge bestehen auf getrennten frischen Datenbanken; Listen und ausgewählte
MCP-Bindungen sind im gemeinsamen Vertrag enthalten. Alle sieben Daemon-Prozess-
prüfer bestehen auf Windows und Linux; Linux zusätzlich mit zwei echten nativen
Schlüssel-Prozessprüfungen. Die endgültigen Rust- und Client-Quellhashes wurden
mit ihren Testmanifesten verglichen.

29 Clienttests, TypeScript und Web-Build bestehen; alle 100 erzeugten Verträge
stimmen überein. Der aktuelle Windows-Desktop-Build besteht. Die sechs nativen
Desktoptests und Clippy sind unverändert geprüft. Die bisherige Linux-WebKit-
Lebenszyklusprüfung bleibt ein historischer Nachweis der nativen Schlüsseleinrichtung;
eine neue interaktive MCP-Desktopabnahme wurde nicht durchgeführt. Die Browser-
prüfung verwendet synthetische API-Antworten und deckt Katalogimport, exakte
Wiederholung, veraltete Auswahl, Fokus, DE/EN, helle/dunkle Darstellung sowie
390/800/1076/1600 Pixel ohne horizontalen Überlauf ab. Die aktuelle Prüfung bestätigt
sichtbare Unsicherheit, gesperrtes Fortsetzen/Abbrechen und escaped Antwortbelege.

MCP-Entdeckung/Credential-Einrichtung, öffentliche belegte Klärung, reale Server,
interaktive native Dialoge, Schlüsselwiederherstellung und Server-KMS bleiben offen.
Einzelbelege: `.evidence/mcp-product-2026-10-08.json`; Verhalten: [MCP-PRODUCT.md](MCP-PRODUCT.md).

| Prüfung | Ergebnis / Grenze |
| --- | --- |
| Rust-Workspace | Historischer persönlicher Inputschnitt: Windows 364/Ubuntu 347 Tests mit Rust 1.94.1, Format/Clippy auf je 184 Quellinputs bestanden. Journal-/Debian-12-Containerberichte bleiben historische Nachweise; keine native GUI-/Enterprise-/M5-Abnahme |
| Rust-Tests | Historischer Journalstand: 348 Windows-/331 Ubuntu-Tests mit Format/Clippy auf identischen 176 Kerninputs; neun PostgreSQL-Verträge und je neun Prozesssuiten auf identischen 186 Prozessinputs. Historische Artefakt-/Produkt-/Desktopberichte bleiben an ihre damaligen Quellen gebunden |
| PostgreSQL | Aktuell elf tatsächliche getrennte Verträge bis Migration 19 auf frischem privatem PostgreSQL 16.15-Cluster bestanden; Cluster bestätigt gestoppt. Frühere neun Journalverträge und Schema16-/17-Berichte bleiben historisch |
| Prozessabbruch | Bestätigte Auftrags- und Workfloweditorentwürfe, unveränderliche Konfliktsnapshots, Pending/Terminal-Vorgangsbelege, Freigaben und Ergebnisse nach hartem Prozessende erhalten; exakte historische Quittungen statt neuer Wirkung. Aktive Workflowclaims werden konservativ unterbrochen; unsichere Aktionen werden nicht wiederholt |
| Prozess-Supervisor | Windows: 13 Tests mit echtem Kindprozess, atomarer Job-Aufnahme, Handle-Liste, Breakaway-Abweisung und Ende bei Supervisor-Abbruch. Linux: zehn Tests mit eigener Prozessgruppe, Parent-Death-Signal und PID-sicherem Reaping. Auf beiden Plattformen Argumente/Umgebung/EOF, Ausgabeflut, binäre Ausgabe, Zeitlimit, Abbruch und Kindende geprüft. Noch keine vollständige Sandbox-Abnahme oder Terminal-Produktanbindung; experimentelle Windows-Isolation separat geprüft. Siehe PROCESS-SUPERVISOR.md |
| Prozessjournal | Acht Tests auf Windows/Linux: alle Policy-Modi, konkurrierende Zulassung/Bestätigung, Lease, Entzug, Arbeitsverzeichnisbesitz bei Abbruch, fehlgeschlagene DB-Transaktionen, manipulierte IDs/Bindungen, Binärbelege, exakter Abgleich und explizite Wiederaufnahme. Ein echter Supervisor-Kindprozess schreibt; Worker wird vor DB-Bestätigung beendet; Wiederöffnung und Abgleich wiederholen die Wirkung nicht. PostgreSQL-Vertrag separat bestanden. Synthetische Adapterberichte, kein Sandbox-Nachweis; siehe PROCESS-JOURNAL.md |
| Idempotenz | 20 gleichzeitige gleiche Startanfragen erzeugen einen Lauf; Start nach Abschluss dedupliziert |
| Historienzweige | 12 konkurrierende gleiche Anfragen erzeugen einen Zweig; fremde Nachricht, Benutzer-Nachricht und veränderte Wiederholung werden abgelehnt |
| SQLite-/PostgreSQL-Historienvertrag | Gleiche Suche, Zweiggrenzen, Herkunft, Aktivitätsfilter und Export-Integrität bestanden |
| Export / Neustart | Python berechnet den Hash des JSON-Exports unabhängig; Snapshot, Suche und Export nach Prozessende erhalten |
| Vorschau-Upgrade (historisch) | Bestehender V2-Preview-Stand nach SQLite-Backup-API-Sicherung auf Migration 9; vollständige Reihenfolge 1–9 und integrity_check nach Wiederöffnung geprüft. 436 Ereignisse, 28 Nachrichten, alle bisherigen Artefakt-/Dateibelege bytegleich; 14 Läufe behalten Zustand und Konfiguration. Lauf-/Ereignisschema 5, Exportformat 7 und 79 Vertragsdefinitionen waren der damalige Stand. Die laufende Vorschau wurde seitdem nicht aktualisiert |
| Umgebungsregister | Windows und Linux: jeweils acht Tests zu versionierten Definitionen, exaktem Replay, konkurrierenden Änderungen, gescheiterten DB-Abschlüssen, alten Referenzen, Rechteentzug, geänderten Runnern und fehlenden Isolationsnachweisen. Bestätigte Aktivierung überlebt hartes Kindprozessende. Volles Journal und Uhr-Rückstellung verhindern den terminalen Entzug nicht. PostgreSQL-Vertrag separat bestanden. Nur synthetische Adapterberichte; siehe ENVIRONMENTS.md |
| Schema 10 | Zusätzliche Umgebungstabelle auf isolierten SQLite-/PostgreSQL-Testdaten geprüft; Migrationsfolge 1–10 und Erhalt historischer Ereignisse/Nachrichten bestätigt. Die laufende Vorschau bleibt auf Schema 9; keine automatische Umstellung |
| Schema / Verträge | Historischer Stand: Migration 19 für owner-scoped Antwortentwürfe/Snapshots/Intents/Quoten nach Editorjournal18 und Attention17; Ereignisschema 7, Exportformat 12 und 161 generierte Verträge. Begrenzter persönlicher Inputschnitt quellengebunden auf Kern/PostgreSQL/Prozess/Frontend/Browser bestanden; input_resume_available=false. Native GUI/WCAG/Enterprise/volle M5-Abnahme offen. Historische 18/7/11- und 144-Vertragsberichte behalten ihren Quellenumfang. Vorschauen/Sidecars nicht automatisch aktualisiert |
| Fehler bei Ergebnis-Insert | Zustand und Ereignis gemeinsam zurückgerollt |
| Explizite Ordnerrechte | Eigene Arbeitsdateien sind auftragsgebunden; zusätzliche Host-Ordner benötigen eine explizite Auswahl; fremde/stale Freigaben, geschützte Daemon-Daten, Traversal, Junctions/Symlinks, Hardlinks und Spezialdateien abgewiesen |
| Native Ordnerverwaltung | Windows: dauerhafte Registrierung, Idempotenz, physische Identität, Rechteentzug über Prozessgrenzen und Neustart, konkurrierende Registrierungen sowie Sperre während laufender IO geprüft. Native HTTP-Routen verlangen zwei getrennte Schlüssel und weisen Browser-Origin vor dem Lesen des Bodys ab; öffentliche Ordnerliste enthält keine Hostpfade |
| Ordner-Prozessprüfung | Ohne folders.json registriert; Neustart, Rechteentzug während Freigabewarten und danach verweigerte Änderung. Zweiter Lauf mit echter Änderung und gescheitertem Beleg: Rechteentzug, Neustart, falscher/richtiger Abgleich und getrennte Fortsetzung ohne Replay. Alte Auswahl-Anfrage aktiviert entzogene Rechte nicht erneut |
| Ordner-Oberfläche | Native Picker-/Bestätigungs- und Verwaltungsimplementierung gebaut; vier IPC-Clienttests bestanden. Browseransicht bei 390/1600 Pixeln, DE/EN, hell/dunkel ohne horizontalen Überlauf; Escape-Fokusrückgabe geprüft. Interaktive native Dialogabnahme noch offen, keine native UI-Steuerung verfügbar |
| Dateimodi und Versionen | Alle vier Modi über die Runtime, Ablehnung ohne Änderung, exakte Freigabe, Versionsinhalt und Hash nach erneutem Öffnen geprüft |
| Schreibbesitz | Wartender zweiter Lauf startet erst nach Freigabe des ersten Besitzers; Besitz überlebt Freigabewarten und Unterbrechung; SQLite/PostgreSQL atomar |
| Unbestätigte Dateiänderung | Echtes Schreiben, absichtlich gescheiterter Datenbankabschluss und Prozessneustart: Marker bleibt, Wiederholung gesperrt, Hash-Abgleich und separate Fortsetzung ohne erneutes Schreiben |
| Datei-Prozesstest | Unter Windows und Linux/WSL2 bestanden; Testprovider spricht echtes lokales HTTP/SSE; Vorversionen, Belege, Idempotenz und unabhängiger Exporthash geprüft |
| Native Verzeichnisoperationen | Windows/Linux-Brokertests: binäre Bäume anlegen/verschieben/entfernen; bestehende Ziele und veränderte Quellen bleiben erhalten; alle vier Policy-Modi, harte Links und Pfadgrenzen geprüft |
| Operations-Prozessprüfung | Windows und Linux-Testcontainer: drei reale Änderungen mit gescheiterten DB-Belegen, Neustart, falscher/richtiger Abgleich, separate Fortsetzung, kein Replay, aufbewahrter Inhalt und unabhängiger Exporthash |
| Operations-Oberfläche | Freigaben, falscher/richtiger Abgleich, gesperrte Fortsetzung, chronologische Belege und Manifest; 390/1600 Pixel ohne horizontalen Überlauf, DE/EN, hell/dunkel. Kleine Ansichten scrollen zusammenhängend; eingefrorene Laufkonfiguration sichtbar |
| Wiederherstellung | Windows: Textversionen und binäre Bäume aus bestätigten Belegen, gleiche Auftrags-/Ordnerrechte, kein Überschreiben, manipulierte Blobs/Manifeste abgewiesen; modellfreier Lauf mit exakter Freigabe und unverändertem Entwurf |
| Wiederherstellungs-Prozessprüfung | Alle Provider aus isolierten Testdaten entfernt; echte Wiederherstellung mit gescheitertem DB-Beleg, Prozessende, falschem/richtigem Abgleich und separater Fortsetzung bestanden; kein erneutes Schreiben und keine Modellanfrage |
| Wiederherstellungs-Oberfläche | Entfernter Ordner nach Freigabe wiederhergestellt; Dateivorschau und Herkunft geprüft, Entwurf erhalten; 390/1600 Pixel ohne horizontalen Überlauf, DE/EN und hell/dunkel |
| Arbeitsdateien und Snapshots | Dauerhafter Dateibestand, binäre Snapshots, getrennte Kopien an älteren Antwortgrenzen, verschachtelte Zweige, beschädigte Inhalte und fremde Workspace-Rechte geprüft |
| Snapshot-Prozessprüfung | Windows und Linux/WSL2: fehlgeschlagener Snapshot-Insert bestätigt kein Ergebnis; Neustart und Fortsetzung ohne zweite Modellanfrage oder Dateiänderung; Dateikopie, Herkunft und Manifesthash unabhängig geprüft |
| Dateidownload | Windows: aktuelle und historische Binärdateien bis zum HTTP-Client nach Neustart unverändert; Herkunft, Größe und Hash geprüft. Fremde Auftrags-/Pfadreferenzen, harte Links, übergroße Dateien, aktive Schreiber und korrupte Versionen abgewiesen |
| Download-/Upload-/Ordner-/Prozess-Client | Historischer 29-Test-Frontendbericht; Binärbytes, leere Dateien, Unicode, falsche Header, Abschneiden/Mehrdaten, Hashfehler, native IPC-Ergebnisse, Abbruch und unklarer Abschluss sowie binärer Upload, Größenlimit, falsche Uploadbestätigung, Pfad-/Schlüsseltrennung bei Ordner-IPC und fehlende automatische Wiederholung geprüft. Neue Prozessverträge weisen zusätzliche Shell-/Environment-Felder und ungültige Bytes ab; historische Laufkonfigurationen bleiben lesbar |
| Download-Oberfläche | Aktuelle/historische Downloadvorbereitung sowie Artefakt-/JSON-Export bei 390/1600 Pixeln, DE/EN, hell/dunkel bedient. Exportstatus auch in Ergebnisansicht sichtbar; Fokusrückgabe geprüft. Browser-Dateiablage weiterhin offen |
| Upload und Import | Windows-Broker, Runtime und HTTP: 2-MiB-Binärdatei jenseits des JSON-Limits, leere Dateien, vier Modi, exakte Freigaben, fremde IDs, Größen-/Hashfehler, beschädigte Blobs, zwei aktive Transfers, veränderte Stages/Ziele und Vorversionen geprüft |
| Upload-Prozessprüfung | Ohne Provider: Reservierung über Neustart, abgebrochener HTTP-Upload mit hartem Prozessende, geflushter Blob mit gescheiterter Metadatenbestätigung, gleicher Upload erneut bestätigt. Import und Ersetzung nach echter Wirkung/gescheitertem Beleg unterbrochen; Neustart, falscher/richtiger Abgleich und separate Fortsetzung ohne Replay; Vorversion wiederherstellbar, Entwurf erhalten, Exporthash unabhängig geprüft |
| Upload-Oberfläche | Echte Browser-Dateiauswahl und Übertragung von Unicode-Binärdatei, Ersatztext und leerer Datei. Zielprüfung, Nur-Lesen-Sperre, ausdrückliche Ersetzung, Freigabe, Wiederherstellung der alten Binärdatei und leerer Import bedient. Bytegenauer unabhängiger Vergleich, drei Uploads und vier modellfreie Läufe mit je einer Freigabe/einem Beleg bestätigt. 390/1076/1600 Pixel, DE/EN, hell/dunkel; eindeutige verschachtelte Dialognamen, Escape-Fokusrückgabe und Fokus nach Zielprüfung geprüft. Native Tauri-Dateiauswahl noch nicht interaktiv abgenommen |
| Nativer Dateischreiber | Windows: echte binäre/leere Dateien, Überschreiben, Abbruch vor Veröffentlichung, geschützte Pfade, Hardlinks, Schreibschutz, veränderte Zielidentität/Inhalte, beschädigte und fremd ersetzte temporäre Dateien geprüft; geschützte Windows-DACL erhalten. Echter Kindprozess vor/nach Speicherbestätigung beendet: ursprüngliche/bestätigte Datei erhalten. Interaktive Dateidialog-, EFS-, Metadaten-, Disk-full- und Stromausfallabnahme offen; siehe NATIVE-SAVE.md |
| Arbeitsdatei-Oberfläche | Freigabe, Textvorschau, Unterordner, historischer Zweig; 390/1600 Pixel ohne horizontalen Überlauf; DE/EN und helle/dunkle Darstellung geprüft |
| Verlorener Worker | Abgelaufene Lease führt zu interrupted; keine automatische Wiederholung |
| Abbruch | Während langsamer Modellanfrage bestätigt |
| Zeitbudget | 1-Sekunden-Budget beendet einen 5-Sekunden-Testprovider vor Antwort; unbeantwortete Freigabe läuft ab; Neustart setzt das Budget nicht zurück |
| Modellbudget | Reservierung vor HTTP, gleichzeitige Zulassung, veraltete Lease, doppelte Abrechnung, gemeldeter/fehlender/partieller Verbrauch, Überlauf und unveränderliche Konfiguration geprüft. Gemeldete Providerüberschreitung blockiert vorgeschlagene Werkzeuge. Preflight und Lauf prüfen denselben Unterhaltungskontext einschließlich Werkzeugdefinitionen |
| Budget-Prozessprüfung | Windows und Linux-Testcontainer: harter Abbruch während Modellanfrage, Neustart, abgelaufene Lease, gehaltene Reservierung und verweigerte neue Anfrage bei ausgeschöpftem Budget. Fehler beim DB-Abschluss nach Antwort behält Reservierung; keine automatische Wiederholung. Exakter Exporthash unabhängig berechnet |
| Budget-Oberfläche | Ungültiger Start erzeugt keinen Lauf und erhält Entwurf; gemessener/partieller Verbrauch, unveränderte ältere Laufgrenzen, Providerüberschreitung ohne Werkzeugwirkung. 390/1076/1600 Pixel, DE/EN und hell/dunkel. Aufgeklappte Einstellungen scrollen mit der Unterhaltung; Kopfzeile bleibt auch beim Feldfokus sichtbar |
| Provider | OpenAI-SSE und Ollama-NDJSON mit lokalen HTTP-Testservern; unvollständige Streams abgelehnt |
| Privater MCP-Transport | 32 neue Prüfungen: Initialize/Session/Version, JSON und SSE einschließlich leerer Eröffnung, Ping und verweigertes Sampling, Katalogseiten und Hashbindung, Offline-Schemata, ungültige Argumente/Ausgaben, doppelte JSON-Schlüssel, Größen-/Nachrichten-/Zeitgrenzen, Redirects, Abbruch nach Wirkung und keine automatische Wiederholung. Dauerhafte MCP-Policy-/Journal-Anbindung und privater Runtime-Einstieg separat ergänzt; OAuth, stdio und Produktoberfläche fehlen weiterhin. Plattformnachweise und Grenzen: MCP-TRANSPORT.md; `.evidence/mcp-transport-2026-10-08.json` |
| MCP-Journal / Runtime | Elf neue Tests je Plattform: vier Modi trotz irreführender Serverannotationen, exakte Argumentfreigabe, parallele Zulassung/Abschluss, alte Lease, Endpoint-/Budgetgrenzen, Entzug, Konfigurationskonflikte, transaktionale Fehler, beschädigte Belege, geschützter Fehlerabschluss und Abbruch. Echter Worker nach dauerhaftem Remote-Append beendet; Wiederöffnung, exakter Abgleich und neue Sitzung wiederholen die Änderung nicht. Derselbe MCP-Speichervertrag auf PostgreSQL bestanden. Private Schnittstelle; keine beliebige Remote-Terminierungs- oder Exactly-once-Garantie. Siehe MCP-JOURNAL.md |
| Verschlüsselte MCP-Sitzungen | Zehn zusätzliche Prüfungen je Plattform: authentifizierte Scope-/Schlüsselbindung, manipulierte Checkpoints, atomare Verbindung/Zulassung/Abschluss, Freigaben, falsche Schlüssel, Sitzungsablauf und drei echte Worker-Abbruchstellen. Bestätigte entfernte Sitzung wird ohne Neuinitialisierung fortgesetzt; unbestätigte Initialisierung und unsicherer Aufruf bleiben gesperrt. Derselbe Verbindungsvertrag separat auf PostgreSQL bestanden. Native/serverseitige Schlüsselverwaltung und Produktintegration offen; siehe MCP-SESSIONS.md |
| Native MCP-Schlüssel / Credentials | Sieben neue reguläre Prüfungen je Plattform: verschlüsselte Scope-Bindung, Revisionen, Rotation/Entzug, fehlende Schlüssel, Manipulation, unveränderte ursprüngliche Sitzungsschlüssel und letzte Zulassungsprüfung. Separater realer OS-Prozessprüfer auf Windows und Linux: Provisionierung, hartes Kindprozessende, Entschlüsselung, gezielte Schlüssellöschung und gesperrter Wiederzugriff. Windows prüft maschinenlokale Persistenz; Linux verwendet ein neues GNOME-Keyring-Profil. Dauerhafter Einrichtungsdienst separat ergänzt; Desktop-Oberfläche, Recovery und Server-KMS offen; siehe MCP-SECRETS.md |
| Native Schlüsseleinrichtung | Acht neue reguläre Prüfungen je Plattform: konkurrierende Reservierungen/Abschlüsse, alte Wiederholungen, Rotation, Aufgeben während laufender Erzeugung, gescheiterte DB-Veröffentlichung, beschädigte Metadaten/Schlüssel, Original-MCP-Sitzung nach Rotation und HTTP-Berechtigungen vor Body-Zugriff. Zusätzlicher echter OS-/SQLite-Prozessprüfer nach bestätigter/fehlgeschlagener Veröffentlichung; PostgreSQL-Speichervertrag bestanden. Desktop-IPC/Bestätigungsoberfläche und Recovery offen; siehe NATIVE-SECRET-SETUP.md |
| Echtes Modell | Ollama qwen3:4b, synthetischer Leseauftrag abgeschlossen; aktuell 1.348 gemeldete Eingabe- und 257 Ausgabe-Token, Reservierung auf 1.605 abgerechnet. Kein Modell heruntergeladen |
| Web | Eingefrorener Input-/Layoutstand 19/7/12: 164 Tests/73 Inputs/sechs Distdateien, TypeScript/161 Verträge/Build und 24 Browserchecks/76 Inputs bestanden. Unterhaltung 552,07 px kompakt/429,74 px aufgeklappt bei 1440×1000; identischer Form-DOM, Tastaturfokus, lostStartACK und echter Neustart. Früherer 145-/20-/263-Inputbeleg und 102-/14-Snapshot-/86-/18-Journalnachweise bleiben historisch. Separater SQL-20-Clocknachweis oben: neue statische DE/EN-Statusprüfung, kein neuer Clock-Browser-/GUI-Durchlauf. Keine vollständige native GUI-/NVDA-/WCAG-Abnahme |
| Browser | Projekt, Auftrag, Start, Freigabe, Ergebnis, Neustart; 1600/1076/390 Pixel |
| Neue Browserabläufe | Zweig erstellen, unabhängig fortsetzen, Eingang → exakte Freigabe, Inhaltssuche; Suchdialog bei 1600/390 Pixeln ohne horizontalen Überlauf |
| Datei-Browserabläufe | Explizite Auswahl, verständliche Dateifreigabe, Versionsvergleich; unsicherer Abschluss blockiert Fortsetzen/Abbruch; falscher Abgleich bleibt Konflikt, passender Abgleich bleibt bis zum ausdrücklichen Fortsetzen unterbrochen; 1600/390 Pixel geprüft |
| Dateiablage des Exports | Historischer Journaltest speicherte seinen bekannten synthetischen Workflowdraft im privaten Browserziel. Im neuen Snapshot-Kopie-Test schloss download.saveAs den Playwright-Kontext unerwartet; keine neue erfolgreiche Browserdateiablage. Stattdessen 6687 tatsächliche Blobbytes und SHA nach Anchor-Navigationsunterdrückung geprüft. Gemeinsame native Save-Bridge implementiert; vollständige OS-Dialogabnahme offen |
| Windows-Desktop | Historischer d0ce-Bericht: sechs native Tests/fmt/clippy/build auf 51 Inputs. Historischer fb975-Lebenszyklus: privater Desktop/Daemon, zwei Rollen nach Fensterende, exakte HTTP-Freigaben und hashgeprüfte Ergebnisse, Reopen gleicher Daemon ohne neue Modellaufrufe; 245 damalige Dateien inklusive sechs Distfiles. Diese Belege qualifizieren nicht die neue Journal-GUI. Native Dialoge/Installer offen |
| V1-Hotfix | Historischer gezielter Einzelpfad: 24 Vitest-Tests und TypeScript-Build, keine Verteilung. Kein vollständiger Crew-Sicherheitsnachweis; vorhandene V1-Crew-Umgehungspfade, umfassende Abnahme und Verteilung bleiben offen |
| Linux-Daemon | Aktueller persönlicher Inputstand: Ubuntu 24.04/WSL2,347 Rusttests und zehn echte Prozesssuiten auf 184 Kern-/195 Prozessinputs getrennt bestanden. Historischer Journalstand176/186/neun Suiten und Verzeichnis-fsync-Fix bleiben erhalten. Keine neue native GUI-/Enterprise-Abnahme |
| Historischer Linux-Kern vor MCP-Produktintegration | Debian 12 im Docker-Testcontainer, UID 10001, keine externen Netzwerkverbindungen, Host-Mounts oder Capabilities: 188 Rust-Tests, Format/Clippy, Daemon-Build und alle sieben aktuellen Prozessprüfer bestanden. Enthält Supervisor, Umgebungsregister, Prozessjournal und verschlüsselte MCP-Sitzungen. Keine Linux-WebView-/gVisor-/Enterprise-Abnahme; ursprüngliche sporadische Testabweichung unten dokumentiert |
| Linux-Desktop | Historischer 55-Test-/57-Frontendstand: Tauri/WebKit in privater Xvfb-/DBus-Sitzung, sechs Tests/fmt/clippy/build und echter Einzelagent-Lebenszyklus nach Fensterende; 231 damalige Inputs/sechs Distfiles. Neuer Journalstand und native Dialoge/SecretService/Screenreader/Installer bleiben separat unqualifiziert |
| Installer / CI | Windows/Linux-Installer, vollständige manuelle Desktop-Abnahme und entfernter CI-Lauf noch offen |
| Enterprise / Migration / WCAG | Nicht abgenommen |

Der echte Modelltest ist kein Benchmark. Tests mit deterministischen Providern beweisen
keine Leistung echter Modellhardware. Die Desktop-Prüfung verwendet Prozessende;
vollständige manuelle Desktop- und NVDA-Abnahmen sind weiterhin erforderlich.

Ein früherer PostgreSQL-Vertrag wurde zusätzlich auf einer neuen, isolierten Datenbank mit Migrationen
1–9 einschließlich Modellreservierung und -abrechnung bestanden. Ein früherer Versuch auf der bereits benutzten Testdatenbank traf auf verbliebene
wartende Testläufe und scheiterte an der Erwartung einer leeren Warteschlange. Die vorherige
Datenbank wurde erhalten. Für Wiederholungen muss eine neue leere Testdatenbank verwendet werden.

Der WSL-Ausfall `HCS_E_CONNECTION_TIMEOUT` betraf den Start der separaten Ubuntu-Distribution.
Die damaligen 52 Rust-Tests und die frühere Desktop-Prüfung bleiben historische Nachweise.
Die danach hinzugekommenen Kern- und Prozessprüfungen sind nun auf Linux im isolierten
Debian-Container bestanden. Dazu gehören IO-Lease, Wiederherstellung, Binärdownload,
native Speicherung mit Unix-Modus-/Symlink-Grenzen, Upload/Import, Ordnerentzug und Modellbudget.
Der damalige Nachweis qualifizierte keinen aktuellen nativen Linux-Desktop. Der neue Desktop-Build-/Lebenszyklusnachweis steht weiter unten; interaktive Dialogabnahme bleibt offen.
Quellmanifest, Image-ID, Daemon-Hash, Limits und Einzelprotokolle liegen unter
`.local/linux-qualification/62d282a3c9e94267b77db580bb9b539c`; siehe
[Linux-Prüfverfahren](LINUX-QUALIFICATION.md). Es wurden keine WSL-Hostdienste neu gestartet.

Die neue Supervisor-Suite liegt getrennt unter
`.local/linux-qualification/7f1ad79a27a9492cb09b38fdb689dfae`: zehn Tests,
Format und Clippy bestanden; alle 83 erfassten Quelldateien stimmen mit dem geprüften
Stand überein. Die unveränderten Daemon-Prozessprüfungen wurden dafür nicht wiederholt.
Unter Windows blieb die laufende Vorschau bestehen; der vollständige Testbuild verwendete
`.local/windows-supervisor-build`, damit deren geöffnete EXE nicht ersetzt werden musste.

Die frühere separate Umgebungssuite liegt unter
`.local/linux-qualification/76774e531f12430f94da5cb05682ee62`: acht neue Tests, zwei
Migrationstests bis Schema 10, Format und Domain-/Storage-Clippy bestanden. Alle 87 erfassten
Quelldateien stimmen mit dem geprüften Stand überein. Die Belege verwenden synthetische
Adapterberichte und ersetzen keine reale Isolationsprüfung. Der PostgreSQL-Vertrag lief
auf einer neuen Datenbank im ausschließlich für V2-Tests verwendeten Container, der danach
wieder gestoppt wurde. Die Vorschau wurde nicht aktualisiert.

Der vorherige vollständige Linux-Stand einschließlich Prozessjournal liegt unter
`.local/linux-qualification/c36c02a6ece142839017c51ad2499b05`: 120 Rust-Tests, Format,
Clippy, Daemon-Build und sieben Prozessprüfer bestanden. Alle 92 Dateien des Quellmanifests
stimmten mit dem damaligen geprüften Code überein. Für diesen Wiederholungslauf wurden
kompilierte Abhängigkeiten aus dem vorherigen Testcontainer in einem privaten Cache-Image
erhalten; Basisimage und Layer-Herkunft wurden geprüft. Alle Tests liefen erneut mit den
gleichen unprivilegierten Containergrenzen. Alte Testergebnisse wurden nicht übernommen.

Der erste vollständige Linux-Versuch unter
`.local/linux-qualification/6fc1ce835c3c4a35a3b6b5eda6b30e96` scheiterte am bestehenden Test
`registration_is_durable_idempotent_scoped_and_revocable_without_restart`: Die Assertion auf
`Conflict` schlug fehl; der ursprüngliche Test gab den tatsächlichen Rückgabewert nicht aus.
Die Assertion wurde um diesen Wert ergänzt. 40 gezielte Wiederholungen und der abschließende
vollständige Lauf bestanden. **Die Ursache der sporadischen Abweichung ist ungeklärt; es wird
kein behobener Brokerfehler behauptet.** Diagnosecontainer:
`cowork-v2-registry-diagnostic-080befa6e7674669bd1789ffa454fabe`.

Vorheriger Prozessjournal-Stand auf Windows: 123 Rust-Tests, Workspace-Clippy und sieben Daemon-Prozessprüfer mit dem getrennten
Buildverzeichnis bestanden. Details stehen in `.evidence/process-journal-2026-10-08.json`.
Die Web-Vertragsprüfung, 20 Tests und der Produktionsbuild bestehen; Vite meldet weiterhin
eine Bundle-Größenwarnung. Die drei PostgreSQL-Verträge verwenden nun auch in der CI-Datei
jeweils eine eigene frische Instanz. Ein entfernter CI-Lauf ist weiterhin offen.

## Historischer Windows-Isolations- und Gesamtstand vor der Workflow-Produktanbindung

Die experimentelle Windows-Erweiterung wurde unter Windows 11 Pro, Build 26200,
mit 139 Workspace-Tests, Formatprüfung und Clippy bestanden. Die 16 zusätzlichen Fälle
bestehen aus 15 Integrationstests und einem Profil-Bereinigungstest. Manifest und Einzelprotokolle:
`.local/windows-isolation-2508f77c0e5b4a8b848d9566db6ff117`.

Ein privater Desktop allein verhinderte OpenInputDesktop nicht; deshalb ist die vor dem
Start geprüfte Win32k-Sperre verbindlich. Die aktuelle Prüfung beobachtet gesperrte
GUI-Initialisierung (1114), verweigerte 512-MiB-Allokation bei 256-MiB-Jobgrenze (1455),
verweigerten zweiten Prozess bei Prozessgrenze eins (1816) sowie 437 ms CPU-Zeit bei
4515 ms Laufzeit unter 100 Millicores. Die Gegenproben für Dateirechte schließen eine andere
aktivierte Kapsel und eine ALL-APPLICATION-PACKAGES-Freigabe ein.

Zwei Beobachtungen bleiben **offene Fähigkeiten**: Winsock scheitert schon bei der
Initialisierung (10107); Kindprozessanlage scheitert auch bei höherer Prozessgrenze (5).
Das sind keine vollständigen Netzwerk- oder Kindprozess-Isolationsnachweise. Einzelprozess-
Abbruch, hartes Ende des Besitzers, Dateisperre, Profilbereinigung und Wiederherstellung ohne
Wiederholung sind geprüft. Festplattenquoten, sichere Ergebnisübernahme, authentifizierte
Runner und Produktanbindung fehlen. Details: [WINDOWS-ISOLATION.md](WINDOWS-ISOLATION.md).

Der damalige Linux-Gesamtstand zur Windows-Isolationsprüfung liegt unter
`.local/linux-qualification/7756bdfc2b774fcc98c1b74c11f42111`: 120 Rust-Tests, Format,
Clippy, Daemon-Build und alle sieben Prozessprüfer bestanden. Das neue Quellmanifest umfasst
99 Dateien; der Container läuft als UID 10001 ohne Netzwerk, Host-Mounts oder Capabilities.
Windows-Prüfungen werden dort nicht ausgeführt. Die oben dokumentierte ältere sporadische
Registry-Abweichung ist weiterhin ungeklärt und wurde nicht als behoben markiert.
Die Windows-Daemon-Prüfer wurden ebenfalls mit einem getrennten Build ausgeführt.
Der neue Bericht mit allen sieben bestandenen Prüfungen liegt unter
`.local/process-windows-4de1a9f207204f28b50969c66c5c1fe6/result.json`.
Die damals 86 generierten Verträge stimmten überein; Schema 11, Ereignisse 6 und Export 8
waren in diesem Prüfschritt unverändert. Frontend-, PostgreSQL- und Desktop-Nachweise stammen aus den oben benannten
vorherigen Prüfungen und sind kein neuer Nachweis für diese Windows-Isolation.

## Vorheriger MCP-Journal-Nachweis

Windows: `.local/mcp-19c092cc8c8046d09d638cfaff332047/result.json` (182 Tests); Linux: `.local/linux-qualification/3d53320c5ca747d5865767c809c9f763/evidence/result.json` (163 Tests). Beide Gesamtprüfungen enthalten Format und Clippy. Windows-Daemon: `.local/process-windows-897d018e377c49eba93cf1968c471e7d/result.json`; Linux-Daemon im genannten Containerbericht. Alle sieben Prozessprüfer bestehen auf beiden Plattformen. Vier PostgreSQL-Verträge auf getrennten frischen Datenbanken: `.local/mcp-postgres-9978e323f29b4394b6ac422488fed778/result.json`; Fixture anschließend gestoppt. Client, TypeScript, Web-Build und Vertragsdrift: `.local/mcp-client-023d2c32c1824128ab763087dd12a411/result.json`.

Die Quellenmanifeste wurden nach Abschluss gegen den aktuellen Arbeitsbaum geprüft. Der Linux-Container verwendete UID 10001, keine Host-Mounts, kein externes Netzwerk, keine Capabilities, No-new-privileges, vier CPUs, 6 GiB und 512 PIDs. Vorhandene kompilierte Abhängigkeiten wurden wiederverwendet; das ist keine unabhängige Release-Reproduzierbarkeitsprüfung. Desktop-Sidecars und laufende Vorschau wurden nicht aktualisiert. Die historische sporadische Registry-Abweichung bleibt offen. Der Web-Build meldet weiterhin einen Hauptchunk über 500 kB.

## Vorheriger MCP-Sitzungs-Nachweis

Windows: `.local/mcp-fe93b44323d04bee9fe448c9dc0edfcf/result.json` (192 Tests); Linux: `.local/linux-qualification/4d3273f540824a7296f42197a17218a5/evidence/result.json` (173 Tests). Format und Clippy bestehen auf beiden Plattformen. Windows-Daemon: `.local/process-windows-f8b40f38da0b49e9992bb61285cd0a3a/result.json`; Linux-Daemon im Containerbericht. Alle sieben Daemon-Prozessprüfer bestehen jeweils. Fünf PostgreSQL-Verträge: `.local/mcp-sessions-postgres-0153b7eb612f4238a082330fc000cfe1/result.json`; frische getrennte Datenbanken, Testcontainer anschließend gestoppt.

Die zehn zusätzlichen Prüfungen umfassen drei Verschlüsselungs-/Bindungsprüfungen, einen Speichervertrag und sechs Runtime-Tests. Echte Worker werden nach bestätigtem Ergebnis, nach dauerhafter entfernter Änderung vor der Antwort und während der Initialisierung beendet. Nur ein bestätigter Checkpoint erlaubt die Fortsetzung derselben entfernten Sitzung. Manipulierte oder veraltete Checkpoints, falsche Schlüssel und fehlgeschlagene DB-Abschlüsse erzeugen keine neue Sendeberechtigung. Details und Grenzen: [MCP-SESSIONS.md](MCP-SESSIONS.md).

Die finalen Quellenmanifeste mit 111 Windows- und 121 Linux-Dateien stimmen mit dem Arbeitsbaum überein. Der Linux-Container lief mit UID 10001, ohne Host-Mounts, externes Netzwerk oder Capabilities, mit No-new-privileges, vier CPUs, 6 GiB und 512 PIDs. Er verwendete bereits kompilierte Abhängigkeiten; dies ist kein Nachweis eines reproduzierbaren Release-Builds. Der Clientbericht aus dem vorherigen Abschnitt bleibt erhalten: seine 13 aufgezeichneten Quelldateien sind unverändert, Frontendtests und Build wurden nicht wiederholt. Die aktuelle Vertragsdriftprüfung erzeugt weiterhin 90 übereinstimmende Definitionen. Die laufende Vorschau und Desktop-Sidecars wurden nicht aktualisiert. Zusammengefasster Nachweis: `.evidence/mcp-sessions-2026-10-08.json`.

## Vorheriger MCP-Credential-Nachweis

Windows: `.local/mcp-4d991ef670544ad3a6e668efab26e1fc/result.json` (199 reguläre Tests); Linux: `.local/linux-qualification/a0795aa6b23549449311e8fd8a8e7c11/evidence/result.json` (180 reguläre Tests). Format und Clippy bestehen auf beiden Plattformen. Der separate native Schlüsselspeicher-Test besteht jeweils zusätzlich; er ist in diesen Summen nicht enthalten. Windows-Gesamtablauf und native Prüfung: `.local/mcp-secrets-windows-6b59605d97404172b971aa90b0f1f697/result.json`. Linux native Prüfung: `native-secrets.log` im genannten Linux-Nachweisverzeichnis.

Windows-Daemon: `.local/process-windows-c856ece75d9a4b02874af09e247acef3/result.json`; Linux-Daemon im Containerbericht. Alle sieben Daemon-Prozessprüfer bestehen jeweils. Sechs PostgreSQL-Verträge auf getrennten neuen Datenbanken: `.local/mcp-secrets-postgres-14cc332f7bde418d96945d16d4d5c4e8/result.json`; Testcontainer anschließend gestoppt. Die aktuelle Driftprüfung bestätigt unverändert 90 generierte Verträge, Ereignisschema 7 und Exportformat 9. Datenbankschema ist 14.

Die finalen Quellmanifeste mit 123 Windows- und 133 Linux-Dateien stimmen mit dem Arbeitsbaum überein. Linux lief mit UID 10001, ohne Host-Mounts oder externes Netzwerk, ohne Capabilities und mit No-new-privileges, vier CPUs, 6 GiB und 512 PIDs. Neue Pakete für DBus/GNOME Keyring und die gesperrten Rust-Abhängigkeiten wurden beim Imagebau installiert; die tatsächliche Prüfung lief offline. Kompilierte frühere Abhängigkeiten wurden weiterverwendet. Der Schlüsselbundtest nutzte einen frischen privaten DBus, temporäres HOME und ausschließlich synthetische Geheimnisse. Dies ist keine manuelle Desktop-Unlock- oder Enterprise-KMS-Abnahme.

Die 13 Quellen des bisherigen Client-Nachweises sind unverändert. Seine 22 Tests, TypeScript und Web-Build wurden nicht wiederholt. Desktop-Sidecars und laufende Vorschau bleiben unverändert; die historische sporadische Registry-Abweichung bleibt offen. Zusammenfassung: `.evidence/mcp-secrets-2026-10-08.json` und [MCP-SECRETS.md](MCP-SECRETS.md).

## Nachweis des unveränderten nativen Einrichtungsdienstes

Windows: `.local/mcp-b8eec78aa2b24087af48c39605a48e44/result.json` (207 reguläre Tests). Linux: `.local/linux-qualification/ef9e16d1442d49fd85528a746b022c3c/evidence/result.json` (188 reguläre Tests). Format und Clippy bestehen auf beiden Plattformen. Die zwei zusätzlichen echten nativen Schlüssel-Prozessprüfer je Plattform sind in diesen Summen nicht enthalten. Windows-Gesamtablauf: `.local/native-setup-windows-50cb6ef88045448a80d136a9d335204c/result.json`; Linux native Prüfung: `native-secrets.log` im genannten Linux-Nachweisverzeichnis.

Windows-Daemon: `.local/process-windows-963314138ae040f6b4c32e6b5d285d56/result.json`; Linux-Daemon im Containerbericht. Alle sieben Prozessprüfer bestehen je Plattform. Sieben PostgreSQL-Verträge: `.local/native-setup-postgres-f038b04ea705423e8bb43ddc08d39d58/result.json`; anschließend wurde der Fixture-Container gestoppt. Client-Nachweis: `.local/native-setup-client-b8bbe2a4011f4a688e94ec6e589b5c3f/result.json` mit 22 erneut ausgeführten Tests, TypeScript und Web-Build. Die Bündelgrößenwarnung bleibt bestehen. 96 generierte Verträge, Datenbankschema 15, Ereignisschema 7 und Exportformat 9 sind geprüft.

Die Quellenmanifeste umfassen 134 Windows-, 144 Linux- und 32 Client-Dateien und stimmen mit dem Arbeitsbaum überein. Der Linux-Test lief ohne Host-Mounts oder externes Netzwerk, mit UID 10001, ohne Capabilities, mit No-new-privileges, vier CPUs, 6 GiB und 512 PIDs. Der Imagebau nutzte den dokumentierten privaten Abhängigkeitscache; die eigentlichen Prüfungen liefen offline. Echte Schlüsseltests nutzten ausschließlich synthetische Einträge und unter Linux einen frischen privaten DBus sowie temporäres HOME/XDG-Profil. Das ist keine manuelle Desktop-Unlock-, Setup-UI- oder Server-KMS-Abnahme.

Vorschau und Desktop-Sidecars wurden nicht aktualisiert. Die historische sporadische Registry-Abweichung bleibt offen. Zusammenfassung: `.evidence/native-secret-setup-2026-10-08.json` und [NATIVE-SECRET-SETUP.md](NATIVE-SECRET-SETUP.md). Vorherige Berichte oben gelten jeweils für ihre damaligen Quellenstände.

Der erste Frontend-Prüfskriptlauf brach beim Drucken eines Vite-Häkchens über CP1252 ab. Nach Umstellung der Berichtsausgabe auf UTF-8 wurden nur Client/TypeScript/Build und die noch ausstehende Vertragsprüfung erneut ausgeführt. Ursprünglicher Fehlerbericht und Log bleiben im Windows-Gesamtnachweis erhalten; dies war kein Produkt-Testfehler.

## Historischer Desktop-Einrichtungsnachweis

Windows: `.local/desktop-secrets-windows-6b8a4a0f09f946b9a124343d05e58e77/result.json`; Linux: `.local/desktop-secrets-linux/24f9fbd046a94995b104e5fffc292c14/evidence/result.json`. Jeweils sechs native Koordinatorprüfungen, Format, Clippy und Build bestanden. Die Prüfungen sichern native Bestätigung/Abbruch, alte Anfragen mit erneuter Argumentprüfung, fehlende automatische Wiederholung, veraltete Revisionen sowie exakt gebundenen Abgleich. Unter Linux startet das echte Frontend den passenden Sidecar; ein angenommener Lauf übersteht das Beenden der WebView und erzeugt genau ein Artefakt. Der Container verwendet UID 10001, kein externes Netzwerk, keine Host-Mounts oder Capabilities, No-new-privileges, vier CPUs, 6 GiB und 512 PIDs. WebKit-Sandbox-Einstellungen wurden nicht deaktiviert.

Client: `.local/native-setup-client-d69ff50fdf5d4cbc9b22e393e3f594fe/result.json`; 27 Tests, TypeScript und Web-Build bestanden. Die synthetische Browserprüfung unter `.evidence/ui/native-secret-setup-ui.json` deckt Browser-Hinweis, native Stornierung, unbekannten Abschluss ohne Replay, Wiederöffnen eines offenen Vorgangs, Abgleich, nicht verfügbaren Schlüssel, Rotation und Aufgeben ab. DE/EN, hell/dunkel, 390/800/1076/1600 CSS-Pixel ohne horizontalen Überlauf; Escape-Fokusrückgabe geprüft. Dies sind simulierte IPC-Antworten, keine tatsächliche Bedienung von OS-Dialogen. Die große Web-Bündelwarnung bleibt bestehen.

46 Windows-Desktop-, 155 Linux- und 36 Client-Quelldateien sind mit ihren Manifesten abgeglichen. Die 134/144 Dateien der bisherigen Windows-/Linux-Kernmanifeste sind unverändert; diese 207/188 Tests wurden nicht wiederholt. Desktoptests sind getrennt von den Kernsummen gezählt. Windows-Vorschau und gestagter Windows-Sidecar wurden nicht ersetzt. Gesamtbeleg: `.evidence/native-secret-ui-2026-10-08.json`.

## Reproduzierbare Befehle

- cargo test --locked --workspace
- cargo clippy --locked --workspace --all-targets -- -D warnings
- node tools/contracts/check.mjs
- npm test
- npm run build
- cargo clippy --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
- cargo test --locked -p cowork-broker --test delivery
- cargo test --locked -p cowork-storage --test processes
- python tests/system/durability.py
- python tests/system/files.py
- python tests/system/workspaces.py
- python tests/system/file_operations.py
- python tests/system/uploads.py
- python tests/system/native_folders.py
- python tests/system/model_budgets.py
- python tools/qualification/linux_container.py
- python tools/qualification/windows_isolation.py
- python tools/qualification/linux_container.py --suite executor
- python tools/qualification/linux_container.py --suite environments
- powershell -NoProfile -ExecutionPolicy Bypass -File tests/system/desktop-lifetime.ps1
- xvfb-run -a python3 tests/system/desktop_lifetime_linux.py --desktop /absoluter/buildpfad/localai-cowork-v2-desktop --daemon /absoluter/repopfad/apps/desktop/src-tauri/binaries/cowork-daemon-x86_64-unknown-linux-gnu
- python tests/system/ollama_smoke.py --data .local/preview --model qwen3:4b

Der Desktop-Test bricht bei belegtem Port 43821 ab und stoppt keine vorhandene Instanz.
Der Ollama-Test lädt kein Modell herunter und verlangt eine ausdrücklich benannte Version.
PostgreSQL separat mit einer neuen leeren Testdatenbank in COWORK_TEST_POSTGRES und
cargo test -p cowork-storage --test contract postgres_same_storage_contract -- --ignored prüfen.

Linux unter WSL wurde mit einem separaten `CARGO_TARGET_DIR` auf dem Linux-Dateisystem
geprüft. Die Python-Prozessprüfungen akzeptieren `--binary` mit dem absoluten Pfad
zum dort gebauten Daemon. Dies ist keine gVisor-, Container-, native Linux-Desktop- oder
Kubernetes-Abnahme. Die separate native Linux-Desktop-Prüfung bestätigt Start und Prozesslebenszyklus
unter Xvfb, jedoch keine manuelle WebView-, Installer- oder Screenreader-Abnahme.
Ein anfänglicher Linux-Fehler durch fsync auf einem O_PATH-Handle wurde
durch erneutes Öffnen von `.` innerhalb der Verzeichnis-Capability behoben; die Tests wurden
danach vollständig bestanden. Ein Fehler beim Bereinigen einer Python-SQLite-Testverbindung
unter Windows wurde durch explizites Schließen der Verbindung korrigiert.

## Private Nachweise

- .reference/v1-0.3.12-complete-source: Quellstand vor Hotfix mit Dateihashes.
- .reference/v1-0.3.12-after-permission-hotfix: erneuter Quellsnapshot mit erweitertem Quellenfilter.
- .evidence/workflow-journal-2026-10-09.json: historischer Journal-Sammelbeleg, sieben bestandene Reports/245 Inputs, dauerhafte Entwürfe und exakte Vorgangsrecovery; M5/Release false.
- .local/workflow-journal-browser-575314f1b8a24f109acc8aaddcc1b3c6: historischer Journal-Browserbericht, sources/dist/proof/stopped, sechs Screenshots und synthetischer Export.
- .evidence/ui: historische lokale Browseransichten.
- .evidence/ui/file-versions-1600.png: geprüfter Vergleich der aufbewahrten Dateiversionen.
- .evidence/ui/workspace-files-390.png: Arbeitsdateien und bestätigter Lauf-Snapshot.
- .evidence/ui/workspace-branch-1600.png: getrennte historische Arbeitskopie.
- .evidence/ui/downloads-de-light-390.png: aktuelle und historische Downloadaktionen.
- .evidence/ui/downloads-en-dark-1600.png: breite englische Downloadansicht.
- .evidence/ui/native-save-web-export-390.png: sichtbare Exportmeldung in der schmalen Ergebnisansicht.
- .evidence/ui/native-save-web-en-dark-1600.png: gemeinsamer Downloadpfad, englische dunkle Ansicht.
- .evidence/ui/upload-import-390.png: Importziel und Zugriffsmodus bei schmalem Fenster.
- .evidence/ui/upload-replace-390.png: ausdrückliche Ersetzung und geprüfte Vorversion.
- .evidence/ui/uploads-en-dark-1600.png: bestätigte Uploads und Arbeitsdateien in breiter englischer Ansicht.
- .evidence/upload-browser-2026-10-08.json: unabhängiger Byte-/Belegabgleich der über den Browser bedienten Importabläufe.
- .evidence/native-folders-2026-10-08.json: Ordnerregistrierung/-entzug, Prozess-, Schema- und Desktop-Nachweise samt offenen Abnahmen.
- .evidence/environments-2026-10-08.json: private Umgebungslifecycle-, SQLite-/PostgreSQL- und Prozessnachweise; synthetische Qualifikationsberichte, keine Sandbox-Abnahme.
- .evidence/model-budgets-2026-10-08.json: Budgetjournal, Browserabläufe, unabhängiger Exportabgleich, realer Ollama-Verbrauch und Schema-9-Upgrade.
- .evidence/linux-core-2026-10-08.json: aktuelle Linux-Kern-/Prozessprüfung einschließlich Containergrenzen und Quellmanifest.
- .evidence/process-supervisor-2026-10-08.json: 13 Windows-/zehn Linux-Prozesstests, vollständige Windows-Workspace-Prüfung und getrenntes Linux-Quellmanifest; keine Sandbox-Abnahme.
- .evidence/windows-isolation-2026-10-08.json: Windows-Grundbausteine, 139 Windows-/120 Linux-Tests, Prozessprüfer, Quellmanifeste und ausdrücklich offene Fähigkeiten.
- .evidence/process-journal-2026-10-08.json: vorheriger Prozessjournal-Stand, 123 Windows-Tests, PostgreSQL-Verträge und Nachweise einschließlich bekannter Grenzen.
- .evidence/native-secret-ui-2026-10-08.json: historischer Desktop-/Clientstand mit je sechs Desktoptests, 27 Clienttests, aktuellem Linux-WebKit-Lebenszyklus und synthetischer UI-Prüfung.
- .evidence/native-secret-setup-2026-10-08.json: unveränderter Kerndienst mit 207 Windows-/188 Linux-Tests, vier separaten echten nativen Schlüssel-Prozessprüfungen, sieben PostgreSQL-Verträgen, 22 Clienttests und geprüften Quellmanifesten.
- .evidence/mcp-secrets-2026-10-08.json: vorheriger Stand mit 199 Windows-/180 Linux-Tests, zwei zusätzlichen echten nativen Schlüssel-Prozessprüfungen, sechs PostgreSQL-Verträgen und geprüften Quellmanifesten.
- .evidence/mcp-sessions-2026-10-08.json: vorheriger Stand mit 192 Windows-/173 Linux-Tests, fünf PostgreSQL-Verträgen, sieben Daemon-Prozessprüfern je Plattform, abgeglichenen Quellmanifesten und verschlüsselten Sitzungs-Checkpoints; kein Produkt- oder Release-Abschluss.
- .evidence/linux-process-journal-2026-10-08.json: vollständige 120-Test-Linux-Prüfung, sieben Daemon-Prozessprüfer, Quellmanifest und tatsächliche Containergrenzen; erste sporadische Registry-Abweichung separat erhalten.
- .evidence/ui/model-budgets-unconfirmed-390.png: zurückgehaltene Reservierung bei unvollständigem Verbrauch.
- .evidence/ui/model-budgets-overrun-en-dark-1076.png: Überschreitungswarnung und gesperrter Lauf.
- .evidence/ui/native-folders-web-de-390.png: schmale Browseransicht ohne native Verwaltungsrechte.
- .evidence/ui/native-folders-web-en-dark-1600.png: breite englische dunkle Ansicht.
- .evidence/ui/restoration-dialog-390.png: Zielwahl für eine modellfreie Wiederherstellung.
- .evidence/ui/restoration-files-390.png: wiederhergestellte Datei und Herkunft.
- .evidence/ui/restoration-files-en-dark-1600.png: breite englische Ergebnisansicht.
- .evidence/ui/folder-grants-390.png: Ordnerauswahl bei schmalem Fenster.
- .local/desktop-smoke-*: eigene Desktop-Testdaten.
- .local/desktop-smoke-7b634ac045a44b1181660eee01a51c99: nativer Dialog-/Speicherbuild, Prozesslebenszyklus ohne interaktiven Speicherdialog.
- .local/desktop-smoke-ac0b7af8fe8f44c3b5d96293ad2b146e: Upload-fähiger Desktop-/Daemon-Aufbau mit bestandenem Prozesslebenszyklus.
- .local/desktop-smoke-3354ebbfb03945f382694de2b28f38c8: Ordnerverwaltungs-Desktop und passender Daemon mit bestandenem Prozesslebenszyklus.
- .local/desktop-smoke-b22bd18dc63140389cbca2cf725c0270: Budget-Desktop und passender Daemon mit bestandenem Prozesslebenszyklus.
- docs/v1-permission-hotfix.patch: isolierte Änderungen gegen den ursprünglichen Arbeitsstand.

Quellarchive haben dokumentierte Ausschlüsse; sie sind keine Sicherung installierter
Anwendungen oder persönlicher Datenbanken.

## Linux-Buildumgebung

Die vorhandene Distribution Ubuntu-24.04 erhielt die für den nativen Build benötigten
WebKit-/GTK-, OpenSSL-, SVG-, Xdo-/Ayatana-Entwicklungsbibliotheken sowie Xvfb/Xauth aus den
Ubuntu-Paketquellen. Dabei wurden unter anderem WebKit und OpenSSL aktualisiert und die
alte AppIndicator-Implementierung durch Ayatana ersetzt. `dpkg --audit` meldet keinen
unvollständigen Paketstand. Rust 1.94.1 wurde über rustup bereitgestellt. Der Linux-Buildcache
liegt getrennt unter `/home/noah/.cache/localai-cowork-v2/target`; private V1-Daten wurden nicht verwendet.
