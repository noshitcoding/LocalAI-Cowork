# Workflow-Produktpfad

## Aktuelle ErgÃ¤nzung: typisierte DatenÃ¼bergaben, 30. Oktober 2026

Der aktuelle Automatisierungsausbau ergÃ¤nzt definierte Workflow-Eingaben,
Ergebnisfelder pro Schritt und explizite Verbindungen. Umfang und aktuelle
PrÃ¼fungen stehen unter [AUTOMATION-IMPLEMENTATION.md](AUTOMATION-IMPLEMENTATION.md).
Die darunterstehenden Abnahmebelege vom 9. Oktober behalten ihren damaligen,
eingefrorenen Umfang; sie sind keine Abnahme des heutigen Gesamtsystems.

1. Einen neuen Ablauf oder eine bearbeitbare Entwurfskopie Ã¶ffnen und
   **Typisierte Ãœbergaben einrichten** wÃ¤hlen. VerÃ¶ffentlichte Versionen werden
   dabei nicht verÃ¤ndert. Dynamische Rollendelegation ist in diesem neuen
   Datenformat noch nicht unterstÃ¼tzt.
2. Unter **Workflow-Eingabefelder** die beim Start benÃ¶tigten Werte anlegen und
   ihren Typ festlegen. Jedes Feld ist verpflichtend. FÃ¼r ein Feld, das keinen
   Wert haben darf, den optionalen Typ wÃ¤hlen und spÃ¤ter ausdrÃ¼cklich null setzen.
3. FÃ¼r jeden Modellschritt dessen **Ergebnisfelder** festlegen. Das Ergebnis ist
   ein Objekt; darunter sind Text, Zahlen, Wahr/Falsch, Objekte und begrenzte
   Listen mÃ¶glich. Die Liste beschreibt Daten, noch keine SchleifenausfÃ¼hrung.
4. Eine **Schritteingabe** ergÃ¤nzen und als Quelle einen festen Wert, ein
   Workflow-Eingabefeld oder das Ergebnis eines vorherigen Schritts auswÃ¤hlen.
   Der Feldpfad `/total` wÃ¤hlt beispielsweise das Feld `total`; ein leerer Pfad
   Ã¼bernimmt den gesamten Wert. Ein Schrittbezug benÃ¶tigt auch die entsprechende
   AblaufabhÃ¤ngigkeit. UngÃ¼ltige Verbindungen vor der VerÃ¶ffentlichung reparieren.
5. Den Ablauf verÃ¶ffentlichen. Im Chat genau diese Version auswÃ¤hlen, die
   Eingabewerte ausfÃ¼llen und den Start vorbereiten. Die StartprÃ¼fung zeigt die
   gebundenen Werte sowie Modelle, Rechte und Grenzen. Erst die BestÃ¤tigung des
   gemeinsamen AusfÃ¼hrungsplans startet den Lauf.
6. FÃ¼r eine gespeicherte Workflowaufgabe die Werte im Aufgabenformular festlegen.
   Diese Werte gehÃ¶ren zu ihrer gespeicherten Revision und gelten fÃ¼r jeden
   Aufgabenstart. Im AuslÃ¶ser kann **Eingaben ausdrÃ¼cklich zuordnen** gewÃ¤hlte
   Felder durch typgeprÃ¼fte Datei-/Webhook-/manuelle Ereigniswerte ersetzen.
   Nicht zugeordnete Werte bleiben fest. Mit **Filter und Eingaben prÃ¼fen** lÃ¤sst
   sich die Projektion auch bei pausiertem AuslÃ¶ser ohne Lauf testen.

Falsche Typen werden nicht stillschweigend umgewandelt. Ein fehlendes Feld oder
ein unpassendes Modellergebnis blockiert den betroffenen Schritt, bevor daraus
ein weiterer Modellaufruf entsteht. Jeder Schritt erhÃ¤lt seine deklarierten
Datenverbindungen; die neue Datenebene verteilt nicht automatisch alle Ã¼brigen
Workflow-Eingaben. AusgewÃ¤hlte Projekt-/Rollenkontexte gelten weiter.
Bestehende AblÃ¤ufe ohne die neue Datenebene behalten ihr bisheriges Format.

## Dokumente in einem Workflow verarbeiten

1. Im veröffentlichten Ablauf die benötigten Werkzeuge `document_read`,
   `document_write` oder `image_edit` ausdrücklich auf den relevanten Rechteebenen
   auswählen. Die Auswahl **Arbeitsbereich des aktuellen Auftrags** macht die
   Vorlage für unterschiedliche Aufträge nutzbar. Schreibaktionen brauchen
   Schreibrechte; im Modus **Nur Lesen** werden sie nicht freigegeben.
2. Die lokale Dokumentengine über die bestehende Werkzeugeinrichtung bereitstellen.
   Eine fehlende Engine verhindert den Start und wird nicht durch ein anderes
   Ausführungsziel ersetzt.
3. Im Chat genau die veröffentlichte Version auswählen und den Start vorbereiten.
   Ist unter **Einstellungen → Sandbox & Umgebungen → Standard-Ausführung**
   **Dokumente und Bilder** gespeichert, wird die Dokumentengine für die
   Planprüfung übernommen. Andernfalls **Dokumentengine verwenden** wählen.
   Die Dokumentengine verwendet die Dokumentprogramme unter
   den Betriebssystem- und Netzwerkrechten des Benutzerkontos. Die Ordnerauswahl
   ist keine Betriebssystemisolation.
4. Anschließend Dateikopie, Modelle, Rechte und Grenzen im genauen Plan prüfen
   und diesen bestätigen. Dokumentfreigaben zeigen die tatsächlichen Quellen,
   Ausgabe und Aktion. Jede Ausgabe wird neu angelegt; vorhandene Dateien
   werden nicht überschrieben.
5. Erzeugte Dateien bleiben als Ergebnis der Arbeitskopie erhalten. Die Übernahme
   in Originale ist eine eigene Aktion. Nach verlorener Startantwort denselben
   gespeicherten Start abgleichen. Bei unterbrochenen Wirkungen zuerst
   **Ausführungsbelege prüfen**; nur vollständig geklärte Läufe lassen sich
   ausdrücklich fortsetzen. Es entsteht dabei keine zweite Dokumentdatei.

Die Enginewahl ist kein Start. Vor der Planbestätigung gibt es keinen Modelllauf
und keine Dokumentveröffentlichung. Neuladen erhält ein bereits gespeichertes
Originalziel. Ändert sich die gebundene Engine, wird der alte Plan nicht ausgeführt.
Automatische Aufgaben und alte Planstarts erhalten daraus keine dauerhafte
Prozessfreigabe; der entsprechende bestätigte Aufgaben-/Regelanschluss bleibt offen.
OCR und native Dokumentknoten sind im unten beschriebenen aktuellen Quellstand
integriert. Die vollständige Rechnungsautomatisierung bleibt im
[Automatisierungsausbau](AUTOMATION-IMPLEMENTATION.md) offen.

## Dokumente im Einzelchat verarbeiten

1. Unter **Einstellungen → Sandbox & Umgebungen → Standard-Ausführung** die
   **Standard-Ausführungsart** festlegen. **Dateien und Text** ist der anfängliche
   Standard; **Dokumente und Bilder** verwendet die Dokumentengine. Die Auswahl
   wird auf diesem Gerät pro verbundenem Arbeitsbereich gespeichert und für neue
   Aufträge direkt in die Planprüfung übernommen. **Bei jedem Auftrag fragen**
   behält die einzelne Auswahl vor der Planprüfung bei. Bereits vorbereitete
   Aufträge verwenden weiterhin ihr gespeichertes Originalziel.
2. Die anschließende Planprüfung zeigt die eingefrorene Dateikopie, den
   Modellanbieter sowie Programm-, Netzwerk- und Schreibrechte. Erst **Plan
   bestätigen und Arbeitskopie starten** beginnt den Lauf. Dokumentprogramme
   verwenden die Betriebssystem- und Netzwerkrechte des Benutzerkontos.
3. Im Freigabemodus die konkrete Dokumentaktion mit Quellen und Ausgabe prüfen.
   Ergebnisse werden neu in der bestätigten Arbeitskopie angelegt; bestehende
   Dateien werden nicht überschrieben. Nur-Lesen erlaubt keine Dokument- oder
   Bildschreibaktionen.
4. Nach verlorener Startantwort **Ursprünglichen Start abgleichen** verwenden.
   Neuladen behält den ursprünglichen Ziel- und Planbezug. Eine neue Zielwahl
   ersetzt keinen bereits vorbereiteten oder angenommenen Start. Bei einem
   unterbrochenen Dateieffekt zuerst die vorhandenen Ausführungsbelege klären
   und danach ausdrücklich denselben Lauf fortsetzen.

Eine fehlende Engine blockiert vor der Modellprobe. Ändert sich die an den Plan
gebundene Engine nach der Prüfung oder Freigabe, wird die alte Dokumentaktion
abgewiesen. Diese Einzelchatwahl erteilt keine dauerhafte Aufgabenfreigabe.
Modellfreie Workflow-Dokumentbausteine, OCR und die vollständige Rechnungsreferenz
bleiben Teil des laufenden [Gesamtauftrags](AUTOMATION-IMPLEMENTATION.md).

## Historischer Produktschnitt vom 9. Oktober 2026

**Begrenzte Input-Layout-Abnahme (9. Oktober 2026):** Der unverÃ¤nderliche Layout-Sammelbeleg (private development evidence; omitted from public sources) bindet sieben Berichte und 265 Eingaben: 164 Frontendtests auf 73 Quellen/sechs Distdateien sowie 24 tatsÃ¤chliche BrowserprÃ¼fschritte auf 76 Quellen. Bei 1440Ã—1000 misst die Unterhaltung 552,07 px mit eingeklappter gesperrter Eingabe und 429,74 px aufgeklappt. Belegzugriff, identischer Formular-/Textarea-DOM, geÃ¶ffnete Einstellungen, Tastaturfokus, verlorene Start-Prepare-ACK und Neustartpersistenz sind geprÃ¼ft. Die fÃ¼nf Backendberichte gelten fÃ¼r ihren eingefrorenen Datenbank-19/Ereignis-7/Export-12/161-Vertragsstand, nicht fÃ¼r die neu begonnene private Clock-/SQL-20-Arbeit. Der 266-Input-Quellsnapshot (private development evidence; omitted from public sources) erhÃ¤lt exakt diese Revision. Der frÃ¼here Input-Sammelbeleg (private development evidence; omitted from public sources) bleibt unverÃ¤ndert historisch (145 Frontendtests/20 Browserchecks/263 Eingaben, Quellsnapshot mit 264 Inputs). VollstÃ¤ndiges M5/M0â€“M10, native GUI, NVDA/WCAG, Enterprise und Release bleiben nicht freigegeben.

**Historische Snapshot-Kopie-Abnahme (2026-10-09):** Historische Vorgangssnapshots lassen sich jetzt nach ausdrÃ¼cklicher PrÃ¼fung als unabhÃ¤ngige Entwurfskopie wiederherstellen. Der separate Frontendbericht (private development evidence; omitted from public sources) besteht 102 ProduktprÃ¼fungen auf 66 Inputs; der Browserbericht (private development evidence; omitted from public sources) besteht 14 begrenzte PrÃ¼fungen auf 69 Inputs und sechs Distdateien, eigener Daemon bestÃ¤tigt gestoppt. Backend/DatenvertrÃ¤ge waren bei dieser historischen Snapshotrevision unverÃ¤ndert; der eingefrorene Input-/Layoutstand ist im separaten Beleg oben abgenommen. Der Journal-Sammelbeleg 2a6e14aâ€¦ bleibt erhalten; seine 86-/18-UI-/Distnachweise sind historisch gegenÃ¼ber dieser Erweiterung. Browser-Dateiablage, native Dialoge, NVDA/WCAG AA, Enterprise und vollstÃ¤ndiges M5 sind nicht abgenommen. Der historische Snapshot-Sammelbeleg (private development evidence; omitted from public sources) ist passed: sieben Berichte, 249 Eingaben, SHA256 bcbd57e6e93937b100447ce135d831c9fd20758b2fd76437ff0e4e3f09f5d425.

Die private V2-Vorschau besitzt jetzt eine bedienbare projektbezogene Ablaufbibliothek,
Ã¶ffentliche `/api/v2`-VertrÃ¤ge sowie einen gemeinsamen Preflight- und Startpfad fÃ¼r
Einzelagenten und native Workflows. â€žÃ–ffentlichâ€œ bezeichnet die authentifizierte
Anwendungs-API des lokalen Einzelbesitzers. Es bezeichnet keinen Ã¶ffentlichen
SaaS-Dienst, keine Teamautorisierung und keine Enterprise-Abnahme.

## Bedienbarer Umfang

Ein Projekt besitzt eine versionierte Bibliothek. Ein Ablauf enthÃ¤lt Rollen mit
explizit ausgewÃ¤hlten Modellprofilen und Profilrevisionen, Anweisungen, Ziele,
AbhÃ¤ngigkeiten, Rechte und gemeinsame beziehungsweise rollen-/schrittbezogene
Budgets. Der Formular-Editor bietet sequenzielle und parallele AblÃ¤ufe sowie die
Fehlerregeln `continue_independent` und `stop`. Neue Rollen oder Werkzeuge erzeugen
keine zusÃ¤tzlichen Rechte. Der effektive Umfang bleibt die Schnittmenge der vom
Dienst erteilten Auftragsrechte, Definition, Rolle und Schritt.

VerÃ¶ffentlichen erzeugt eine neue unverÃ¤nderliche Version. Bereits angenommene
LÃ¤ufe behalten ihre gespeicherte Bindung. Bei einer konkurrierenden VerÃ¶ffentlichung
bleibt der Editorentwurf sichtbar; der aktuelle Stand muss ausdrÃ¼cklich geprÃ¼ft
und als neue Basis gewÃ¤hlt werden. Sperren verlangt eine sichtbare BestÃ¤tigung.
Der Entzug sperrt neue Starts und neue Aktionen, wÃ¤hrend bereits bestÃ¤tigte
Ergebnisse und historische Versionen erhalten bleiben.

UnvollstÃ¤ndige Editorformulare werden Ã¼ber den Anwendungsdienst dauerhaft gespeichert,
mit Formatkennung, Basisrevision und unverÃ¤nderlichem Snapshot. Verlorene
Save-Antworten behalten denselben eingefrorenen Request. JSON-ObjektschlÃ¼ssel werden
fÃ¼r den Vergleich kanonisch geordnet; eine vom Dienst anders angeordnete Antwort
ist keine neue Benutzereingabe. Ein CAS-Konflikt erhÃ¤lt beide Fassungen. Die
eingehende Kopie kann bewusst als unabhÃ¤ngiger editierbarer Entwurf geÃ¶ffnet werden.
Unbekannte/beschÃ¤digte Formate bleiben nur lesbar und Ã¼ber den gemeinsamen
Downloadpfad exportierbar. Kollidierende Rollennamen werden sichtbar zurÃ¼ckgewiesen;
beim Reparieren schon mehrdeutiger Namen verlangt jeder betroffene Schritt eine
ausdrÃ¼ckliche Neuzuordnung. Die OberflÃ¤che rÃ¤t keine Modell-/Rollenzuordnung.

Der Composer wÃ¤hlt ausdrÃ¼cklich â€žEinzelagentâ€œ oder einen konkreten verÃ¶ffentlichten
Workflowstand. Die Vorschau zeigt Rollen, Modellnamen, Endpunkte, Werkzeugrechte
und Grenzen. Provider-Credentials werden dabei nicht angezeigt. Der Dienst prÃ¼ft
alle gepinnten Modelle einschlieÃŸlich spÃ¤terer Rollen; er wÃ¤hlt keinen Ersatzanbieter.

Dieser Produktschnitt fÃ¼hrt Modelle, die Datenbankwerkzeuge `create_artifact`,
`list_artifacts` und `read_artifact` sowie gebundene Agentenfragen Ã¼ber
`request_input` aus. Artefaktzugriff bezieht sich auf den autorisierten Auftrag
einschlieÃŸlich verifizierter geerbter Branch-Ergebnisse. Fragen bleiben je Knoten,
Turn und Capabilityfingerprint getrennt; Antworten Ã¤ndern keine Rechte.
Host-Dateien, MCP, Prozesse, Browser und Office werden durch die Workflow-Auswahl
nicht freigeschaltet. Der native Compiler kann weitergehende Definitionen validieren;
das bedeutet nicht, dass der Produktpfad deren AusfÃ¼hrung zulÃ¤sst.

## Gemeinsame VertrÃ¤ge

`cowork-workflow-contracts` erweitert die bestehenden Rust-VertrÃ¤ge um die aus der
DomÃ¤ne abgeleitete Workflowdefinition und die Ã¶ffentliche Produkt-Facade. Die
Generierung erzeugt gemeinsam JSON Schema, OpenAPI, TypeScript und CSP-kompatible
Validatoren. Es gibt derzeit 161 Definitionen einschlieÃŸlich Editor-/Vorgangs-
und Antwortentwurfs-/IntentvertrÃ¤gen; die browserseitige Facade enthÃ¤lt
keine private Plan-/Checkpointstruktur, Worker-Lease oder AusfÃ¼hrungsberechtigung.

| Operation | Vertrag und Verhalten |
| --- | --- |
| `GET /projects/{project}/workflows` | `WorkflowPage` mit projektbezogenen EintrÃ¤gen, Gesamtanzahl und nÃ¤chstem Offset; Standardseite 30, serverseitiges Limit 1â€“100 |
| `POST /projects/{project}/workflows` | `CreateWorkflow` mit IdempotenzschlÃ¼ssel und Definition; liefert `WorkflowLibraryEntry` |
| `GET /projects/{project}/workflows/{workflow}` | Metadaten, Revision, Sperrstatus und Versionreferenzen |
| `GET /projects/{project}/workflows/{workflow}/versions/{version}` | UnverÃ¤nderliche Definition und Referenz mit Hash |
| `POST /projects/{project}/workflows/{workflow}/versions` | `PublishWorkflow` mit Basisrevision, IdempotenzschlÃ¼ssel und Definition; liefert `WorkflowPublication` |
| `POST /projects/{project}/workflows/{workflow}/revoke` | `RevokeWorkflow` mit Basisrevision und IdempotenzschlÃ¼ssel; liefert einen gespeicherten Publikationsbeleg |
| `POST /preflight` | `RunRequest` wird als bisheriger `CreateRun` oder als `WorkflowRunRequest` geprÃ¼ft; Antwort `RunPreflight` |
| `POST /runs` | Derselbe Requestvertrag; Workflowstart fÃ¼hrt aktuelle PrÃ¼fungen erneut durch und liefert den gemeinsamen `RunRecord` |
| `GET /runs/{run}/workflow` | Autorisierte `WorkflowRunView` mit bestÃ¤tigtem Parent, Bindung, Rollen/Modellen, Knoten, Nutzung, aktiven exakten Freigaben, gebundenen Fragen/Receipts und gesondertem input_resume_available=false |
| `POST /runs/{run}/approvals/{approval}` | Bestehender Freigabepfad mit exaktem Fingerprint und Entscheidung |

Die direkten Bibliotheks-/Startendpunkte bleiben kompatibel. Die OberflÃ¤che verwendet
fÃ¼r Erstellen, Publizieren, Sperren und Workflowstart jetzt den dauerhaften
Prepare/Commit-Pfad. Commit fÃ¼hrt dieselben aktuellen Start-/PolicyprÃ¼fungen aus;
die Aktionsroute nimmt keine neue Konfiguration entgegen. Entwurfs-, Snapshot-,
Vorgangs- und Acknowledge-Routen sind in [WORKFLOW-JOURNAL.md](WORKFLOW-JOURNAL.md)
beschrieben. Antwortentwurf/Snapshot/Prepare/ID-Commit-VertrÃ¤ge sind getrennt
in [WORKFLOW-INPUTS.md](WORKFLOW-INPUTS.md) beschrieben: strikt bodyloser Commit,
kein Antwort-Acknowledge oder Discard, keine automatische Sendekette. Einzelagenten
behalten ihren bestehenden Start-/Wiederholungspfad; deren Inputroute nimmt keine
Workflowantworten an.

Alle Pfade besitzen das PrÃ¤fix `/api/v2`. Die bestehende Einzelagent-Requestform bleibt
erhalten. Ein Workflowrequest verwendet `kind: "workflow"`, Auftrags-ID,
Basisrevision, IdempotenzschlÃ¼ssel, Prompt, Workflow-ID, konkrete Versions-ID,
Zugriffsmodus und Budgets. Workflowdefinitionen verwenden den Modustag `mode`,
beispielsweise `{ "mode": "parallel", "max_parallel": 2 }`; `kind` ist kein
Definitionstag.

Der Browser sendet weder einen frei konstruierten `RunSpec` noch Werkzeuge,
Dateifreigaben, Policyrevision, Executor oder Workerrechte. Der Runtime-Dienst
leitet den vertrauenswÃ¼rdigen lokalen Umfang ab und begrenzt diesen Produktschnitt
auf hÃ¶chstens vier parallele Agenten. Das ist keine globale Provider-/Teamquote.
`expected_binding` vergleicht optional die bestÃ¤tigte Definition-/Planbindung aus
dem Preflight; sie kann keine Rechte erteilen. Der Start prÃ¼ft Provider,
Revisionen, Credentials, Grenzen und Bindung erneut. Preflight nimmt keinen Lauf
an und erteilt keine AusfÃ¼hrungsberechtigung.

Ein identischer bereits angenommener Start liefert seinen gespeicherten Lauf auch,
wenn Profil, Auftragsrevision oder Bibliothek inzwischen geÃ¤ndert wurden. GeÃ¤nderte
Inhalte mit demselben SchlÃ¼ssel bleiben ein Konflikt. Nach einem unklaren
Acknowledgement lÃ¤dt die OberflÃ¤che den dauerhaft gespeicherten Vorgang samt
ursprÃ¼nglicher Absicht. Commit, Discard und Acknowledge verwenden ausschlieÃŸlich
dessen ID; eine neue Headrevision ersetzt keinen historischen Annahmebeleg.
Vor neuen SchlÃ¼sseln lÃ¤dt sie alle unbestÃ¤tigten Belege im Projekt/Auftragskontext.
Ein bereits angenommener Lauf wird durch Discard nicht abgebrochen.

## Freigaben, Ereignisse und Eingang

Ein Workflow-Parent bleibt `running`, wÃ¤hrend ein einzelner Agent auf Freigabe
wartet. Die ProduktoberflÃ¤che liest daher den separaten Workflowview und bindet
die Freigabe an Rolle, Schrittziel und konkrete Aktion. Eine Guard auf
`waiting_approval` des Parents wÃ¼rde diese Entscheidungen verstecken und wird
fÃ¼r Workflowfreigaben nicht verwendet. UnabhÃ¤ngige Agenten kÃ¶nnen weiterarbeiten.

Modellstreams verschiedener Agenten werden nicht zu einer einzigen Chatantwort
vermischt. Die Ansicht zeigt getrennte KnotenzustÃ¤nde und Modellzuordnungen;
das bestÃ¤tigte gemeinsame Endergebnis erscheint Ã¼ber den normalen Auftragsverlauf.
Interne IDs, Fingerprints und Protokolldaten befinden sich in geschlossenen Diagnosen.

Migration 17 ergÃ¤nzt `workflow_attention` als abgeleiteten, dauerhaft gespeicherten
Hinweisindex. Parent-/Workflowrevision, aktuelle Zulassung und frÃ¼heste Frist
begrenzen die HandlungsfÃ¤higkeit. Die gemeinsame AktivitÃ¤ts-/Eingangsabfrage
berÃ¼cksichtigt aktive Workflowfreigaben auch bei einem laufenden Parent. Entzug,
Abbruch, Stop, abgelaufene Rechte und bereits entschiedene Freigaben dÃ¼rfen daraus
keine neue Aktion erzeugen. Der Index ist weder Zustandsbesitzer noch Freigabequelle;
die tatsÃ¤chliche Entscheidung prÃ¼ft weiterhin den gemeinsamen Policy-/Leasepfad.

Ein unterbrochener Graph erhÃ¤lt keine allgemeine WiederholungsschaltflÃ¤che mit
neuer Modellanfrage. Die OberflÃ¤che erlaubt ausschlieÃŸlich den vom Dienst bestÃ¤tigten
`completion_resume_available`-Fall: Abschluss bereits terminaler bestÃ¤tigter
Ergebnisse ohne neue Modell-/WerkzeugausfÃ¼hrung. Unsichere frÃ¼here Aufrufe bleiben
reserviert und werden nicht automatisch erneut gesendet.

## Dauerhafte Agentenfragen und Antwortbelege

Der Workflowview zeigt mehrere unabhÃ¤ngige Fragen mit Rolle, Schritt, Frist,
Fingerprint, actionable und bestÃ¤tigtem Receipt. WÃ¤hrend einer Frage bleibt der
Parent Running mit Lease/Heartbeats. request_input benÃ¶tigt im Modus ConfirmAll
zuerst die exakte Werkzeugfreigabe. Antworten gehÃ¶ren ausschlieÃŸlich in den
Werkzeugverlauf des fragenden Agenten; exakte UTF-8-Bytes einschlieÃŸlich Leerraum
und ZeilenumbrÃ¼chen bleiben erhalten.

SingleOwnerPreview-EntwÃ¼rfe erlauben leeren/unkompletten Text, CAS und immutable
Konfliktkopien. Prepare verwendet nur eine bestÃ¤tigte Revision und friert sie ein.
Der ausdrÃ¼ckliche bodylose ID-Commit bestÃ¤tigt Workflowfortschritt, Agentenreceipt,
Humanreceipt, konkretes Ereignis und Accepted-Intent in einer Transaktion. Lost ACK
verwendet den ursprÃ¼nglichen gespeicherten Vorgang; historische Accepted-Replays
verursachen keine neue Wirkung. Unbekannte Unterbrechung bleibt Pending; ein
geprÃ¼fter dauerhafter Abbruchbeleg sperrt neue Antworten und EntwurfsschreibvorgÃ¤nge
auch nach konservativer Interrupted-Ãœbernahme. input_resume_available bleibt false.
Der eingefrorene persÃ¶nliche Input-/Layoutstand 19/7/12 ist im
Layout-Sammelbeleg (private development evidence; omitted from public sources) mit sieben Berichten/265 Eingaben bestanden.
Die fÃ¼nf Backendberichte binden Windows 364/Ubuntu 347 RustprÃ¼fungen, Format/Clippy,
elf PostgreSQL-VertrÃ¤ge und zehn Prozesssuiten je Plattform an ihre unverÃ¤nderte
Revision. Der neue Frontendbericht (private development evidence; omitted from public sources) besteht 164 Tests/73 Inputs/sechs
Distdateien und der Browserbericht (private development evidence; omitted from public sources) 24 reale Checks/76 Inputs.
Private Clock-/SQL-20-Arbeit ist begonnen und dadurch nicht qualifiziert.

### Platz fÃ¼r Unterhaltung und RÃ¼ckfragen

Der angenommene Start besitzt einen kurzen, ausdrÃ¼cklich aufklappbaren Beleg.
Die gesperrte Eingabe ist nur bei exaktem bestÃ¤tigt gebundenem Start ohne Fehler,
Loading, Konflikt oder unbestÃ¤tigtem Entwurf kompakt. Historische abweichende
angezeigte LÃ¤ufe komprimieren sie nicht. Originalauftrag, Modelle, Rechte, Budgets
und Diagnosen bleiben erreichbar; ein Aufklappen erzeugt keine Aktion oder Rechte.
Der Formularbaum bleibt Ã¼ber Pendingâ†’Accepted identisch, einschlieÃŸlich der
geÃ¶ffneten Budgetdetails. Enter und sichtbarer Fokus sind tatsÃ¤chlich geprÃ¼ft.

Bei 1440Ã—1000 stehen 552,07 px mit eingeklappter Eingabe und 429,74 px aufgeklappt
fÃ¼r Unterhaltung/RÃ¼ckfragen bereit. Die vorherige 90-px-LÃ¼cke ist damit fÃ¼r diesen
gebundenen Stand behoben. Die 389,74 px-Vorprobe (private development evidence; omitted from public sources) bleibt failed;
nach der gezielten 320-px-HÃ¶henbegrenzung folgt eine frische vollstÃ¤ndige Abnahme.
Bei 390 px gibt es keinen horizontalen BodyÃ¼berlauf. Verlorene Start-Prepare-ACK
bleibt sichtbar und Reload startet nichts automatisch; der echte Neustart erhÃ¤lt
AntwortentwÃ¼rfe und eingefrorene Pending-VorgÃ¤nge und schreibgeschÃ¼tzte Accepted-Belege ohne Modellreplay.
Die 266-Input-Sicherung (private development evidence; omitted from public sources) erhÃ¤lt exakt den qualifizierten Stand.
Native GUI, Zoom-/NVDA-/WCAG-Gesamtabnahme, Enterprise, M5 und Release bleiben offen.

## Historischen Ausgangsstand als unabhÃ¤ngige Kopie erhalten

Ein nicht bestÃ¤tigter terminaler Create-/Publish-Vorgang bietet â€žAusgangsstand prÃ¼fenâ€œ.
Die OberflÃ¤che liest den Vorgang erneut und prÃ¼ft Projekt, VorgangsidentitÃ¤t und dessen
unverÃ¤nderte Request-/Snapshotbindung. AnschlieÃŸend liest sie genau Draft-ID, Snapshot-ID
und SHA-Referenz. Der Dienst verifiziert den vollstÃ¤ndigen Inhaltshash; die OberflÃ¤che
ersetzt ihn nicht durch einen vermeintlich universellen JavaScript-JSON-Hash.

Bearbeitbare Editorformate mÃ¼ssen semantisch der eingefrorenen Definition entsprechen.
Dieser Vergleich ergÃ¤nzt Budgetdefaults und sortiert/dedupliziert Setfelder fÃ¼r Rechte
und Delegation. Rollen-, Schritt- und AbhÃ¤ngigkeitsvektoren behalten ihre Reihenfolge.
Die gespeicherten Editordaten werden dabei nicht normalisiert oder Ã¼berschrieben.
Bei Publish mÃ¼ssen Herkunft, Workflow und Basisrevision passen; die ursprÃ¼ngliche
Version wird zusÃ¤tzlich im gleichen Projekt mit expliziter Antwort-ID-PrÃ¼fung gelesen.
Pending bleibt bei der vorhandenen Recovery. Unbekannte Formate oder falsche Bindungen
bleiben nur lesbar; reine PrÃ¼fung erzeugt weder Save noch Acknowledge.

â€žAls unabhÃ¤ngigen Entwurf speichernâ€œ erzeugt ausdrÃ¼cklich eine neue Draft-ID/CAS0 und
einen einzigen eingefrorenen Save-SchlÃ¼ssel. Der bisherige Head bleibt unverÃ¤ndert.
Ziele, Modelle und ursprÃ¼nglicher Versionsbezug bleiben in der Kopie erhalten. Der
ursprÃ¼ngliche Vorgang wird separat bestÃ¤tigt; dessen Ack entfernt die neue Kopie nicht.
Bei verlorenem Save-Acknowledgement wiederholt die OberflÃ¤che dieselbe Kopier-ID und
Anfrage. Bei Quote bleiben Eingaben und Exportinhalt erhalten, ohne eine nicht bestÃ¤tigte
Speicherung als dauerhaft auszugeben. Eine andere VerÃ¶ffentlichungsbasis wird zuerst
nur angezeigt und erst nach ausdrÃ¼cklicher Ãœbernahme gespeichert. Ein anderes Modell
wird ebenfalls ausdrÃ¼cklich gewÃ¤hlt. StrictMode-Setup/Cleanup und alte asynchrone
Projekt-/VorgangslesevorgÃ¤nge sind durch Session-/Sequenzgrenzen abgesichert.

Der historische Snapshot-Frontendbericht (private development evidence; omitted from public sources)
besteht 102 ProduktprÃ¼fungen/66 Inputs plus TypeScript, Vertragsdrift und Build. Der
Browserbericht (private development evidence; omitted from public sources)
besteht 14 gezielte PrÃ¼fungen/69 Inputs/sechs Distdateien, Quellen und Daemon unverÃ¤ndert,
eigener Prozess gestoppt. TatsÃ¤chlich bedient wurden Kopie â†’ verlorenes Save-Ack â†’
separates ursprÃ¼ngliches Ack â†’ identischer Retry, alter Snapshot gegen neueren Head,
ausdrÃ¼cklicher Basiswechsel, echte 32-Draft-Quote, CAS-Konfliktkopien, readonly/pending,
spÃ¤te Projektantworten, Tastaturfokus und lesbare Untertitel bei 390px.

Der Download-saveAs-Versuch schloss den Playwright-Kontext unerwartet. Er bleibt als
Vorversuch erhalten. Die ErsatzprÃ¼fung erfasste 6687 tatsÃ¤chlich erzeugte Blobbytes
und SHA256 d0d96abb7c537ed5ee1470ce91ed1f4c0199e1ca5f81fdc0fd5a085957b3e033;
nur die Anchor-Navigation wurde zur InhaltsprÃ¼fung unterdrÃ¼ckt. Das qualifiziert den
Exportinhalt, keine neue Browser-Dateiablage. CSS-VergrÃ¶ÃŸerung um 200 Prozent bleibt
eine begrenzte Beobachtung; native Zoom-/Dialog-/NVDA-/WCAG-Abnahme bleibt offen.

## PrÃ¼fung und Betrieb

Der tatsÃ¤chlich erzeugte und validierte
Journal-Sammelbeleg (private development evidence; omitted from public sources)
enthÃ¤lt sieben bestandene Berichte, 245 erneut geprÃ¼fte Inputs in der Quellenvereinigung
und acht gehashte historische Vorversuche. Die 176 Kerninputs sind zwischen Windows,
Ubuntu und PostgreSQL identisch; die 186 Prozessinputs stimmen zwischen Windows und
Ubuntu Ã¼berein. fullM5Acceptance, fullPlanComplete und productionReleaseAllowed bleiben false.
Collector-SHA256: 2a6e14a0e47fc69d25f6795c96761f37b94c2b23e64c37b0f642eb315d290d36.

Der historische Snapshot-Kopie-Frontendbericht
.local/workflow-snapshot-client-a6d01eb73eda4835863804bfaeb3ce2f/result.json
besteht 102 ProduktprÃ¼fungen, TypeScript, Vertragsdrift und Web-Build auf 66 Inputs.
Manifest-SHA256: 0abcf96327bdac98433f0dd539853824465f50ca6a6c7ce8bbad070042701a73.
Der frÃ¼here Journal-Frontendbericht mit 86 PrÃ¼fungen/63 Inputs bleibt historisch.
Die explizite Testauswahl erfasst Produktdateien aus apps/packages/tests/tools;
55 historische Desktopkopietests unter .local zÃ¤hlen nicht mit. FrÃ¼here
140-/149-ZÃ¤hler sind daher keine aktuellen Produktzahlen.

Historische 348 Windows-/331 Ubuntu-Kerntests mit Format/Clippy, neun PostgreSQL-VertrÃ¤ge bis
Migration 18 und neun echte Daemon-Prozesssuiten je Plattform sind separat bestanden.
Die PostgreSQL-Fixture wurde bestÃ¤tigt gestoppt. Die damaligen Lauf-/ExportvertrÃ¤ge verwendeten
Ereignisschema 7 und Exportformat 11. Der aktuelle Inputstand verwendet Datenbank 19,
Ereignisschema 7 und Exportformat 12; er braucht eigene vollstÃ¤ndige Nachweise. Eine Ã¤ltere Schema-17-PrÃ¼fung
qualifiziert die Journalmigration nicht. Exakte Reportpfade stehen in [VERIFY.md](VERIFY.md).

Die frÃ¼here quellengebundene Journal-Browserprobe
.local/workflow-journal-browser-575314f1b8a24f109acc8aaddcc1b3c6/result.json
besteht 18 PrÃ¼fungen auf 65 Inputs und sechs Distdateien. Sie ist an den qualifizierten
Daemon 20526d0a6a3171039ebdc353730aa25879166d96c29a3c04ac33eb6caa8379d8 gebunden.
UnvollstÃ¤ndige EntwÃ¼rfe Ã¼berstehen Reload; schmutzige Dialoge bleiben bei Escape offen,
sauberes SchlieÃŸen stellt Fokus wieder her. Ein unbekanntes Format bleibt nur lesbar
und sein bekanntes synthetisches Exportdokument wurde tatsÃ¤chlich im privaten
Fixtureverzeichnis gespeichert und bytebezogen geprÃ¼ft.

Drei absichtlich verlorene bestÃ¤tigte Antworten prÃ¼fen Prepare, Publikation und Start.
WiederÃ¶ffnung zeigt dieselbe Pending-/Accepted-Absicht ohne neuen SchlÃ¼ssel.
Der angenommene Publikationsbeleg Ã¶ffnet seine originale Version 2 auch nach einem
separat erzeugten Serverstand 3. Der historische Version-1-Name bleibt im Composer
korrekt. Der Start Ã¶ffnet denselben Lauf; zwei exakte Rollenfreigaben bei laufendem
Parent liefern zwei hashgeprÃ¼fte Artefakte und genau eine gemeinsame Antwort.
Acknowledge/Reload erzeugen weder einen zweiten Lauf noch weitere Modellaufrufe
neben den vier ursprÃ¼nglichen synthetischen Anfragen.

Ein echter CAS-Konflikt erhÃ¤lt die Serverfassung und die eingehende Immutable-Kopie.
Die ausdrÃ¼cklich gewÃ¤hlte unabhÃ¤ngige Entwurfskopie entspricht exakt dem Snapshot.
WorkingFiles initialisiert, aktualisiert und Ã¶ffnet erneut mit HTTP 200; die beiden
historischen Artefakte bleiben sichtbar. Budgetvorschau verwendet 262144 Gesamttoken.
create_artifact zeigt den festen Typ text/plain; charset=utf-8. read_artifact benutzt
arguments.id; dessen Anzeigevertrag wird im qualifizierten SSR-Test geprÃ¼ft,
vom create-only Browserprovider jedoch nicht interaktiv ausgefÃ¼hrt.

Die Probe erfasst Konsole/Netzwerk vor dem Stop. Drei ERR_FAILED-Meldungen gehÃ¶ren den
gezielten AntwortabbrÃ¼chen, ein HTTP 409 dem echten erhaltenen Konflikt.
Es gibt keine unerwarteten API-, JavaScript- oder HTML-Patternfehler.
Der Browser wurde vor dem eigenen Daemon geschlossen; ownDaemonStopped und abschlieÃŸende
Quellen-/Dist-/Binarygleichheit sind bestÃ¤tigt. DE/EN, 1440/390 Pixel und begrenzte
Tastatur-/Fokusbedienung sind geprÃ¼ft. CSS-VergrÃ¶ÃŸerung auf 200 Prozent bleibt eine
Simulation; HintergrundÃ¼berschriften-Reflow und die enge Draftlisten-Unterzeile sind
keine bestandene WCAG-Abnahme. NVDA, tatsÃ¤chlicher Browserzoom und native Dialoge bleiben offen.

## Historische Produkt- und Desktopnachweise

Der frÃ¼here
Workflow-Produktbeleg (private development evidence; omitted from public sources)
bleibt auf den damaligen Quellenstand beschrÃ¤nkt: 55 Frontendtests/57 Inputs,
323 Windows-/306 Ubuntu-Kerntests, acht PostgreSQL-VertrÃ¤ge und acht Prozesssuiten
je Plattform. Die damaligen Windows-/Linuxdesktopberichte bestanden jeweils sechs
Tests, Format/Clippy/Build. Linux fÃ¼hrte das echte WebKit-Frontend unter Xvfb aus
und bewies den Einzelagent-Lebenszyklus nach FensterschlieÃŸen.

Der separate historische
Windows-Workflow-Lebenszyklusbericht (private development evidence; omitted from public sources)
enthÃ¤lt 245 damalige Dateien einschlieÃŸlich sechs Distdateien. Ein privater
Desktopbuild mit ausschlieÃŸlich leerem Cargo-Workspace-Marker fÃ¼hrte zwei Rollen
nach Desktopende fort; zwei HTTP-Freigaben und zwei hashgeprÃ¼fte Artefakte,
Reopen am gleichen damaligen Daemon ohne neue Modellaufrufe. Eigene PIDs/WebViews
wurden bereinigt; ursprÃ¼ngliche Vorschau und Sidecar blieben unverÃ¤ndert.
Diese Nachweise qualifizieren weder die neue JournaloberflÃ¤che/Dist noch GUI-Klicks,
native Speicherdialoge, Secret Service, Installer oder vollstÃ¤ndige Desktopbedienung.

Die damaligen Browserberichte dedc2b609ab448cdaa0b9f2ae24b2758 und
982b84caf60b4f4eb17dd0af9df094bf unter .local/workflow-browser-... behalten
VerÃ¶ffentlichungs-/Freigabe- und WorkingFiles-Init/Remount-Umfang. Budgetdefaults,
HTML-v-Pattern, Immutable-Versionsnamen, Medienanzeige und serialisierte
Dateiabfragen wurden durch diese frÃ¼heren Proben korrigiert. FrÃ¼here private
Artefakt-/Queue-/Koordinatorbelege qualifizieren keine spÃ¤tere API/UI automatisch.

Der erste Journal-Browserbericht
.local/workflow-journal-browser-2a8b331aa3804f568aa37441a573a523/result.json
ist fehlgeschlagen: dieselbe uneditierte unvollstÃ¤ndige Form wurde wegen
JSON-SchlÃ¼sselreihenfolge 106-mal gespeichert. Der damalige Prozess wurde sauber
gestoppt und der Bericht unverÃ¤ndert erhalten. Kanonischer ordinaler Objektvergleich,
Regression, neue Frontendqualification und tatsÃ¤chliche Browserwiederholung lÃ¶sen
diesen konkreten Fehler; historische 140-/149-Testbelege werden nicht umgedeutet.

FÃ¼r Tests nur neue V2-Verzeichnisse und getrennte Datenbanken verwenden.
Eine Schema-18-Quelle ist kein Datenbankrollback auf Schema 17.
V1-Daten und Liveinstallationen bleiben getrennt; Vorschauen oder Sidecars wurden nicht ersetzt.

## Verbleibende Anforderungen

- Quoten bleiben sichtbar; bestÃ¤tigte/aufgegebene Historie zÃ¤hlt weiter, automatische
  Bereinigung fehlt. Eine wegen Quote abgewiesene neue Kopie bleibt im Editor erhalten
  und exportierbar, ist ohne bestÃ¤tigten Save aber nicht dauerhaft gespeichert. Die
  Snapshot-Kopie ist fÃ¼r offene terminale Create-/Publish-Belege verfÃ¼gbar; Pending
  verwendet die vorhandene Recovery. Native Exportdialoge bleiben unqualifiziert.
- Hierarchische Modellsteuerung, externe Datei-/MCP-/Prozesswerkzeuge, dauerhaft
  Input-Wiederaufnahme, strukturierte Ausgaben, vollstÃ¤ndige Abbruch-/AblaufprÃ¼fung
  und allgemeiner belegter Abgleich beziehungsweise Agentenresume bleiben offen.
- Vorlagenbibliothek, Skills, Wissen/Memory, Zeitplanung und Umgebungsinstaller
  schlieÃŸen das vollstÃ¤ndige M5-Paket noch nicht ab.
- Reale Modellqualifikation, aktuelle vollstÃ¤ndige Desktop-/WebView-Bedienung, native Dialoge,
  Barrierefreiheit, Teams, globale Quoten, Offlinebetrieb, Compose/Kubernetes,
  Migration, Wiederherstellung und Enterprise-Last sind getrennte unerfÃ¼llte
  Anforderungen des freigegebenen Gesamtplans.

Diese Produktintegration schlieÃŸt weder M5 noch M0â€“M10 ab und erlaubt keine
produktive Umstellung oder VerÃ¶ffentlichung.

## Bedingungen, Zweige und ZusammenfÃ¼hrung

Im Workfloweditor lassen sich unter â€žBedingungen und ZusammenfÃ¼hrungâ€œ native
Bausteine ergÃ¤nzen. Sie benÃ¶tigen kein Modell. FÃ¼r einen reinen Kontrollablauf
zuerst einen Baustein hinzufÃ¼gen; danach kÃ¶nnen der bisherige Modellschritt und
seine unbenutzte Rolle entfernt werden. Die typisierten Workflow- und
Schritteingaben stehen im anschlieÃŸenden Datenbereich.

Eine Bedingung liest eine benannte Schritteingabe und vergleicht sie mit einem
Wert gleichen Typs. Die Auswahl â€žSchritte nur bei einer Bedingung ausfÃ¼hrenâ€œ bindet
den gewÃ¼nschten vorherigen Bedingungsbaustein und wahr oder falsch. Der Editor
ergÃ¤nzt dabei die nÃ¶tige AbhÃ¤ngigkeit. Nicht gewÃ¤hlte Schritte erscheinen im
Lauf als â€žÃœbersprungenâ€œ mit BegrÃ¼ndung; sie senden keine Modellanfrage.

Ein Datenbaustein erzeugt ein Objekt aus seinen Eingaben. Eine Auswahl verbindet
die gleich typisierten Ergebnisse alternativer Zweige und benÃ¶tigt genau ein
vorhandenes Ergebnis. Eine Sammlung erzeugt eine Liste in der sichtbaren
Quellenreihenfolge. Beide kÃ¶nnen das gesamte Ergebnis oder einen Feldpfad
Ã¼bernehmen. Fehlende Typen, zyklische AbhÃ¤ngigkeiten und ungÃ¼ltige Quellen mÃ¼ssen
vor VerÃ¶ffentlichung korrigiert werden. Nach dem Entfernen eines Knotens bleiben
ungÃ¼ltige Daten-/BedingungsbezÃ¼ge zur ausdrÃ¼cklichen Reparatur sichtbar.

Die Bausteine verwenden die bestehende VerÃ¶ffentlichung und StartbestÃ¤tigung.
Reine KontrollablÃ¤ufe zeigen dort â€žModellanfragen: Keineâ€œ. Gespeicherte Ergebnisse
und Ã¼bersprungene Schritte bleiben nach Neustart erhalten; eine unterbrochene
AusfÃ¼hrung wird Ã¼ber die vorhandene ausdrÃ¼ckliche Fortsetzung weitergefÃ¼hrt.
Die bisherigen Regeln fÃ¼r unklare externe Wirkungen gelten weiter.

Absolute Termine und Ereigniswarten gehÃ¶ren zum weiterhin offenen Ausbau.
Ã–ffentliche Listenverarbeitung ist im unten beschriebenen Format5-Pfad verfÃ¼gbar.
Eingefrorene UnterablÃ¤ufe sind im unten beschriebenen Format4-Produktpfad verfÃ¼gbar.
Sammlung ist noch keine Schleife Ã¼ber ListeneintrÃ¤ge. PrÃ¼fungen und Grenzen stehen
in [AUTOMATION-IMPLEMENTATION.md](AUTOMATION-IMPLEMENTATION.md).

Bei gemischten Workflows prÃ¼ft der Start weiterhin alle deklarierten Modellprofile,
auch wenn die spÃ¤tere Bedingung einen Modellzweig Ã¼berspringt. FÃ¼r einen reinen
Kontrollablauf werden keine Modellrolle, Verbindung oder Zugangsdaten benÃ¶tigt.

## Dauerhafte Wartezeiten

â€žWarteschritt hinzufÃ¼genâ€œ ergÃ¤nzt einen Baustein mit einer Wartezeit in ganzen
Sekunden. Der Wert kann fest eingegeben oder im Datenbereich aus einer
Workfloweingabe beziehungsweise einem frÃ¼heren Ergebnis Ã¼bernommen werden.
ZulÃ¤ssig sind 1 bis 86.400 Sekunden innerhalb des verbleibenden Zeitbudgets.
Bereits bei der Vorbereitung bekannte ungÃ¼ltige Werte verhindern den Start.
Der Editor lÃ¤sst bei festen Werten einen zusÃ¤tzlichen Zeitpuffer; auch die
vorherigen Schritte und die Wartezeit verbrauchen das ursprÃ¼ngliche Zeitbudget.

Der Baustein benÃ¶tigt kein Modell und keine Token. Sobald nur noch Timer und
deren abhÃ¤ngige Schritte warten, wird der Worker fÃ¼r andere AuftrÃ¤ge frei.
Der Lauf bleibt in der AktivitÃ¤tsÃ¼bersicht sichtbar. Seine Ansicht zeigt den
gespeicherten Fortsetzungstermin in der lokalen Zeitzone. Nach dem Termin erhalten
nachfolgende Schritte das Ergebnis `{"wake_at": ...}` mit UTC-Sekunden.

Die Wartezeit wird dauerhaft einmal festgelegt. Das SchlieÃŸen der Ansicht oder
ein Neustart des Daemons beginnt sie nicht erneut. Der laufende Daemon setzt
denselben Lauf automatisch fort; war er zum Termin ausgeschaltet, erfolgt die
Fortsetzung beim nÃ¤chsten Start innerhalb der noch gÃ¼ltigen Rechte und Fristen.
Ein abgebrochener, widerrufener oder abgelaufener Lauf startet keine Folgeschritte.
Bei unsicherer oder zurÃ¼ckspringender Systemzeit bleibt die Fortsetzung gesperrt.

Vorhandene Ordnersperren bleiben wÃ¤hrend der Pause bestehen. Andere AuftrÃ¤ge
kÃ¶nnen freie Ordner verwenden. Der Timer gibt weder zusÃ¤tzliche Dateirechte frei
noch verlÃ¤ngert er Freigaben. MehrtÃ¤giger Betrieb und Warten auf externe Ereignisse
sind weiterhin eigene offene Ausbauschritte.

## UnterablÃ¤ufe aus verÃ¶ffentlichten Vorlagen

Im Editor unter â€žUnterablÃ¤ufeâ€œ einen Workflow und eine konkrete verÃ¶ffentlichte
Projektversion auswÃ¤hlen. Den RÃ¼ckgabeschritt und optional ein RÃ¼ckgabefeld
festlegen; anschlieÃŸend â€žVersion als Unterablauf kopierenâ€œ wÃ¤hlen. Die Kopie ist
unabhÃ¤ngig: spÃ¤tere Ã„nderungen der Vorlage verÃ¤ndern den eigenen Ablauf nicht.

Die Eingaben dieses Aufrufs im Datenbereich ausdrÃ¼cklich mit Workflow-Eingaben,
Konstanten oder vorherigen Ergebnissen verbinden. Die Namen und Typen kommen aus
der kopierten Definition. Fehlende oder geÃ¤nderte BezÃ¼ge bleiben zur Reparatur
sichtbar. Eigene Rechte und Budgets kÃ¶nnen die Grenzen des Elternlaufs und des
Bodys weiter einschrÃ¤nken; sÃ¤mtliche Aufrufe teilen zusÃ¤tzlich dessen Restbudget.

Neue UnterablÃ¤ufe verwenden einen Graphen mit ausdrÃ¼cklichen AbhÃ¤ngigkeiten. Beim
HinzufÃ¼gen aus einem sequenziellen Entwurf werden dessen bisherigen Schrittfolgen
als Kanten erhalten. Der Lauf zeigt den Unterablauf samt enthaltenen Schritten,
Wartezeiten und zusammengefasstem Verbrauch. VerÃ¶ffentlichen und Starten verwenden
das bestehende Journal und die gemeinsame bestÃ¤tigte AusfÃ¼hrung mit Arbeitskopie.

Aktuell gelten insgesamt64 erzeugte Knoten,32 Rollen,16 Aufrufe und hÃ¶chstens vier
Verschachtelungsebenen fÃ¼r statisch eingefrorene UnterablÃ¤ufe. Hierarchische
Komposition bleibt im Ausbau. FÃ¼r ListeneintrÃ¤ge gelten zusÃ¤tzlich die unten
beschriebenen dynamischen Grenzen. PrÃ¼fstand und Grenzen stehen im Automatisierungsnachweis.

## Listen aus Eingaben oder Ergebnissen verarbeiten

Zuerst im Datenbereich eine typisierte Liste als Workfloweingabe oder Ergebnis
festlegen. Unter â€žListen verarbeitenâ€œ eine verÃ¶ffentlichte Projektversion ohne
weitere Listen wÃ¤hlen. Liste, Ergebnisschritt und RÃ¼ckgabefeld pro Eintrag festlegen
und mit â€žVersion als Listenablauf kopierenâ€œ eine unabhÃ¤ngige Kopie erzeugen.
SpÃ¤tere Ã„nderungen der ursprÃ¼nglichen Projektvorlage verÃ¤ndern diese Kopie nicht.

â€žEingaben pro Eintragâ€œ verbindet jedes Vorlagenfeld ausdrÃ¼cklich mit dem aktuellen
Eintrag, einem Feld darin, seinem ursprÃ¼nglichen Index ab 0, einer gemeinsam
Ã¼bergebenen Eingabe oder einem festen Wert. ZusÃ¤tzliche gemeinsame Eingaben werden
im Datenbereich des Listenbausteins verbunden. Andere Elternwerte und gleichnamige
Felder werden nicht automatisch in die Vorlage Ã¼bernommen.

â€žGleichzeitig aktive EintrÃ¤geâ€œ begrenzt die Zahl noch nicht abgeschlossener
EintrÃ¤ge. Wartezeiten und ausstehende Freigaben behalten ihren Platz. Bei einem
Platz werden EintrÃ¤ge nacheinander in Eingangsreihenfolge zugelassen; darÃ¼ber
hinaus gelten die ParallelitÃ¤ts- und Restbudgetgrenzen des gesamten Ablaufs.
Rechte und Grenzen der Liste und der eingefrorenen Vorlage kÃ¶nnen diese weiter
einschrÃ¤nken. Es gibt keinen zusÃ¤tzlichen Lauf oder Budgettopf pro Eintrag.

Das Ergebnis ist eine Liste der ausgewÃ¤hlten RÃ¼ckgaben in ursprÃ¼nglicher
Eingangsreihenfolge. Doppelte Eingangswerte bleiben getrennte EintrÃ¤ge. Eine leere
Liste erzeugt keine Vorlageninstanz. Fehlende, Ã¼bersprungene oder falsch typisierte
Ergebnisse werden nicht still aus der Ergebnisliste entfernt. Nachfolgende
Bausteine mÃ¼ssen den Listenbaustein als AbhÃ¤ngigkeit und Datenquelle binden.

ListenentwÃ¼rfe bleiben im bestehenden Journal wiederherstellbar. VerÃ¶ffentlichung
und Start verwenden weiterhin den gemeinsamen bestÃ¤tigten Plan mit Arbeitskopie.
Die Laufansicht gruppiert EintrÃ¤ge nach ihrem ursprÃ¼nglichen Index und zeigt
Wartezeiten sowie den gemeinsamen Verbrauch. Ein Neustart beginnt bereits
festgelegte Wartezeiten nicht neu; ungeklÃ¤rte Modellantworten werden nicht erneut
gesendet.

Neue Listenquellen verwenden Definitionsformat5. UnterstÃ¼tzt sind bis 16 flache
Listenbausteine im Elternworkflow, jeweils mit einer typisierten Format2-/3-/4-
Vorlage einschlieÃŸlich vorhandener UnterablÃ¤ufe. Verschachtelte Listen und
hierarchische Listenvorlagen sind gesperrt. Typen begrenzen Listen auf hÃ¶chstens 1000
EintrÃ¤ge; der tatsÃ¤chliche Graph ist zusÃ¤tzlich auf 4096 Knoten und 32 MiB begrenzt.
Die unverÃ¤nderten Schritt-/Token-/Zeitgrenzen kÃ¶nnen die ausfÃ¼hrbare Anzahl weiter
begrenzen. Ein vollstÃ¤ndiger1000-Eintrag-Lauf und dessen Leistungsabnahme bleiben
offen. Der genaue [PrÃ¼fstand](AUTOMATION-IMPLEMENTATION.md) ersetzt keine allgemeine
Desktop-, PostgreSQL-, Enterprise- oder Releaseabnahme.


## Vorlagen mit dem Arbeitsbereich des aktuellen Auftrags

Die Auswahl â€žArbeitsbereich des aktuellen Auftragsâ€œ macht einen Workflow fÃ¼r
verschiedene AuftrÃ¤ge verwendbar. Sie wird beim bestÃ¤tigten Start an den
verwalteten Arbeitsbereich des gewÃ¤hlten Auftrags gebunden. Dieselbe verÃ¶ffentlichte
Vorlage kann dadurch in zwei AuftrÃ¤gen jeweils deren ausgewÃ¤hlte Arbeitskopie
verwenden. Vorhandene konkrete Ordnerkennungen behalten ihre ursprÃ¼ngliche Bedeutung.

Die Auswahl im Workflow und bei jeder Rolle beziehungsweise jedem Schritt
aktivieren, der dort Dateien verwenden soll. Die benÃ¶tigten Dateiwerkzeuge mÃ¼ssen
zusÃ¤tzlich ausgewÃ¤hlt sein. Bei eingefrorenen UnterablÃ¤ufen und Listen muss die
Vorlage die Auswahl bereits enthalten; der Aufruf beziehungsweise Listenbaustein
kann sie zusÃ¤tzlich einschrÃ¤nken. Die Auswahl ist nur verfÃ¼gbar, wenn sie auch in
den Ã¼bergeordneten Rechten ausgewÃ¤hlt ist. Das Kopieren einer Vorlage Ã¼bernimmt
keine zusÃ¤tzlichen Rechte des ursprÃ¼nglichen Auftrags.

Danach den Lauf vorbereiten, die Quelle und Arbeitskopie prÃ¼fen und den Plan
bestÃ¤tigen. Die Auswahl erteilt selbst keine Dateifreigabe. Ohne eindeutig
verfÃ¼gbaren verwalteten Arbeitsbereich lÃ¤sst sich der Ablauf nicht starten.
Werkzeugrechte, Ã¼bergeordnete Grenzen, aktuelle Freigaben und das ursprÃ¼ngliche
Budget gelten weiterhin. Eine spÃ¤tere Ã„nderung der Originaldateien verÃ¤ndert die
bereits gewÃ¤hlte Eingabekopie nicht.

Die Auswahl bleibt beim Speichern, WiederÃ¶ffnen und VerÃ¶ffentlichen des Entwurfs
erhalten. Die Zusammenfassung zeigt sie auch an eingefrorenen Vorlagen, Rollen,
Schritten und Aufrufen. Ein fehlender Arbeitsbereich wird als Einrichtungsproblem
angezeigt. Alte `workspace:<order>`-Kennungen werden beim Laden oder Kopieren
nicht automatisch auf einen anderen Auftrag Ã¼bertragen.


## Native Dokumentaktionen ohne Modellkonto: aktueller Quellstand

Die Bibliothek bietet „Dokumentablauf ohne Modell“ als eigenen Entwurfseinstieg.
Die vorgegebene DOCX-Aktion benötigt nur einen neuen Dateinamen und ihren Inhalt.
Unter „Dokumentaktionen ohne Modell“ können Textlesen, Seitenvorschau und
DOCX-/PDF-Erstellung ausgewählt werden. Die typisierten Datenübergaben zeigen
deren feste Eingabefelder und unveränderliche Ergebnistypen. Dokumentaktionen
benötigen keine Modellrolle; Freigaben benennen den betreffenden Schritt.

Für die Ausführung wird weiterhin ausdrücklich die Dokumentengine gewählt und
die Arbeitskopie mit den tatsächlichen Benutzer- und Netzwerkrechten bestätigt.
Die Auswahl des aktuellen Arbeitsbereichs ist eine logische Auswahl; die
Ausführung bindet sie an genau die bestätigte Kopie. Vorhandene Dateien werden
nicht überschrieben. Vorschauen zeigen ihre Seitenzuordnung und markieren
gekürzte Ergebnisse; lange Texte behalten einen Unicode-Fortsetzungsmarker.

Dieser Quellstand besitzt43 aktuelle Windows-Backend- und608 Frontendfälle.
Eine aktuelle Browser-/Installationsabnahme und eine vollständige Linuxprüfung
sind damit nicht behauptet; Ubuntu-Gesamtläufe bleiben wegen Zeit-/Budget-
Unterbrechungen9/12. Die vollständigen Belege und verbleibenden Grenzen stehen
im aktuellen Abschnitt von [AUTOMATION-IMPLEMENTATION](AUTOMATION-IMPLEMENTATION.md).
OCR, Rechnungsreferenz und Prozessfreigaben für gespeicherte Regeln bleiben offen.

Nachfolgende enge Browserprüfung: Ein neuer modellfreier DOCX-Ablauf wurde im
Browser tatsächlich bearbeitet, veröffentlicht, mit ausdrücklicher
Dokumentengine und bestätigter Quellkopie gestartet und genau einmal freigegeben.
Eine verlorene Freigabeantwort, Neuladen und ein harter Neustart nach Abschluss
behielten denselben Lauf und dieselbe Datei. JSON-Ergebnisse bleiben jetzt als
Daten mit unveränderten Feldnamen und leeren Listen lesbar. Die aktuelle
Main-Oberflächenprüfung besteht631 Fälle, Typprüfung und Webbuild; die vollständige
Linux-, Installations- und Gesamtabnahme bleibt offen. Die neue Linuxdiagnose
zeichnete tatsächliche Systemuhr-Rücksprünge auf. Genaue Quellen und Grenzen
stehen im nachfolgenden Browserabschnitt der Implementierungsdokumentation.


## OCR und Excel-Tabellen: aktueller Quellstand vom 1. November 2026

Native Dokumentaktionen enthalten jetzt lokale Texterkennung für Deutsch und
Englisch sowie **Excel-Tabelle ergänzen**. Die oben dokumentierten historischen
Prüfgrenzen bleiben an ihre damaligen Quellen gebunden. Der aktuelle Umfang
steht in OCR-TABLE-VALIDATION.json (development artifact omitted from public sources).

Für OCR die zusätzliche Dokumentengine samt Sprachpaketen einrichten. Im
Editor Sprachen, Seiten, Auflösung und Drehung wählen; Quellpfad und
Inhaltsfingerprint ausdrücklich verbinden. Ohne Seitenauswahl werden höchstens
die ersten acht Seiten erkannt. Ergebnisse enthalten Wortpositionen, den
Quell- und Rasterfingerprint und Rohwerte von 0 bis 100; diese Werte sind keine
Wahrscheinlichkeiten. Weitere Wörter oder Seiten ausdrücklich fortsetzen.

Für die Tabellenbuchung Excel-Blatt und benannte Tabelle auswählen. Spalten
in Excel-Reihenfolge mit exakten Überschriften, Datenfeldern und Typen
konfigurieren. Ein bis vier nicht leere Schlüsselfelder verhindern doppelte
Zeilen in der Quelle und im aktuellen Stapel. Höchstens 100 Zeilen je Aktion
und 32 Spalten sind möglich. Geldbeträge verwenden ganze Centwerte:
11900 wird als numerischer Wert 119,00 gespeichert. Datumswerte benötigen
JJJJ-MM-TT. Fehlende Werte erfordern ausdrücklich erlaubtes null.

Quelldatei, Fingerprint, neue Ausgabedatei und typisierte Zeilenliste unten
verbinden. Die Aktion prüft die gespeicherten Zellwerte und Typen erneut.
Vorhandene Dateien bleiben erhalten; die neue Ergebnisversion vor einem
weiteren Eingang ausdrücklich als nächsten Tabellenstand übernehmen. Tabellen
mit Summen-/berechneten Spalten, Schutz, überlappenden Bereichen oder belegten
Zielzellen benötigen weitere Bearbeitung und werden nicht stillschweigend
verändert. Bestehende Formelausdrücke bleiben erhalten, berechnete Werte
benötigen gegebenenfalls einen ausdrücklichen Neuberechnungsschritt.

Dokumentengine und Arbeitskopie im Startplan bewusst bestätigen. Im
Freigabemodus zusätzlich die konkrete Schreibaktion prüfen. Nach verlorener
Antwort oder unterbrochener Veröffentlichung zuerst die ursprünglichen
Ausführungsbelege klären und denselben Lauf ausdrücklich fortsetzen. Die
Tabellenprüfung belegt Wiederaufnahme nach Neustart ohne zweite Datei
oder Buchung; sie ersetzt keine durchgängige Rechnungsautomatisierung.
Extraktion, persistente Übernahme, Ablage und Meldung bleiben Teil des
Gesamtauftrags. Das inzwischen integrierte Korrekturformular wird im folgenden
Abschnitt beschrieben.

## Prüfformulare ohne Modell: aktueller Quellstand vom 1. November 2026

In der Ablaufbibliothek **Prüfformular ohne Modell** wählen oder in einem
bestehenden Ablauf **Prüfformular hinzufügen**. Frage, Feldnamen, lesbare
Beschriftungen und Hinweise bearbeiten. Dieser Baustein benötigt keine
Modellrolle und ruft selbst kein Modell auf.

Feldtypen und Darstellung ausdrücklich wählen. **Kalenderdatum** prüft echte
Kalenderdaten. **Geldbetrag mit zwei Nachkommastellen** verarbeitet Punkt oder
Komma ohne Rundung als ganze Centwerte. Für Summen eine Regel mit mindestens
zwei Summanden und einem getrennten Summenfeld anlegen; bei Rechnungen etwa
Nettobetrag plus Steuerbetrag gleich Bruttobetrag. Grenzen und erforderliche
Angaben werden auch auf dem Dienst geprüft.

Ein fehlender Extraktionswert kann als **Optionaler Wert (oder null)**
vorbelegt sein und zugleich **Angabe erforderlich** verlangen. Das Formular
zeigt dann ein direkt bearbeitbares leeres Feld. Nach der Bestätigung erhält
ein Folgeschritt den festen Ergebnistyp; ein benötigter Centbetrag bleibt
keine nullable Ausgabe. Wahr/Falsch-Felder verlangen eine ausdrückliche Wahl.

Vorbelegungen unten im Datenbereich mit festen Werten, Workflow-Eingaben oder
Ergebnissen vorheriger Schritte verbinden. Neue Formulare benötigen keine
zusätzlichen Workflow-Eingabefelder. Änderungen veröffentlichen eine neue
Version; bereits gestartete Läufe behalten ihren gespeicherten Stand.

Im laufenden Auftrag Angaben ergänzen oder korrigieren. Unvollständige
Entwürfe werden gespeichert. **Antwort zur Prüfung vorbereiten** friert den
geprüften Entwurf ein; erst **Geprüfte Antwort senden** bestätigt ihn und gibt
den Folgeschritt frei. Nach einer verlorenen Antwort denselben Vorgang
abgleichen. Ein Neustart erzeugt keine zweite Frage oder Bestätigung. Ein
unterbrochener wartender Lauf verlangt die ausdrückliche Fortsetzung seines
ursprünglichen Vorgangs; seine Frist wird dabei nicht verlängert.

Der Baustein besitzt ausschließlich das Rückfragerecht. Formularwerte
erteilen keine Datei-, Netzwerk- oder Werkzeugrechte. Für eine nachfolgende
Excel-Buchung gelten deren eigener bestätigter Plan und Aktionsfreigaben.

Der aktuelle gebundene Prüfstand (development artifact omitted from public sources) belegt
723 Frontendtests, Typprüfung/Webbuild, 748 Verträge und ausgewählte
Windows-/Linux-SQLite- sowie native Queue-/Neustartfälle. Im tatsächlichen
Main-Browserweg wurden ein neues Formular und Version 2 veröffentlicht,
fehlende Werte und eine falsche Summe behandelt und die ursprüngliche
bestätigte Antwort nach Dienstneustart automatisch geladen. Eine installierte
Desktop-App, PostgreSQL-Formularabnahme, mobile Freigaben und die vollständige
Rechnungsautomatisierung sind damit nicht abgenommen. Die zugehörige
Quellenansicht und Extraktionsstarter sind inzwischen im Main-Quellstand
implementiert. Die [Bedienung und aktuellen Grenzen](DOCUMENT-EXTRACTION.md)
beschreiben den neuen dreistufigen Ablauf und die inzwischen geprüfte
Kontextregel **Nur verbundene Eingaben**. Sie gilt für neu veröffentlichte,
ausdrücklich gewählte Versionen; ältere Versionen behalten ihren bisherigen
Kontextmodus. Der neue Main-Browsernachweis (development artifact omitted from public sources)
besteht mit kurzem Windows-Laufzeitpfad einschließlich nativer Korrektur,
einmaliger Antwortbestätigung und automatischer Quellen-/Antwortanzeige nach
Dienstneustart. Der erste fehlgeschlagene Langpfadversuch bleibt erhalten;
Windows-Pfadlängen und eine verständliche Vorabprüfung bleiben offen.
