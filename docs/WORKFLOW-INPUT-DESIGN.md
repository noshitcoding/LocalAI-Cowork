# Workflow-Rückfragen — Entwurf des nächsten M5-Schnitts

Stand: 9. Oktober 2026. **Entwurf, noch nicht implementiert oder abgenommen.**

Dieses Dokument beschreibt den nächsten begrenzten Produktschnitt: `request_input`
in nativen sequenziellen und parallelen Workflows, dienstseitig dauerhafte
Antwortentwürfe, unverändert erhaltene Antworten und eine ausdrücklich begrenzte
Wiederaufnahme bestätigter menschlicher Wartecursor. Allgemeines Workflow-Resume,
zusätzliche externe Werkzeuge und Enterprise-/Offlineabnahme bleiben offen.
Dieser Entwurf erteilt keine M0-, M5- oder M0–M10-Gesamtabnahme.

Die Befunde stammen aus lokalen Quelllesungen. Produktquellen und Verträge wurden
für diesen Entwurf nicht geändert; neue Tests, Builds oder Prozess-/
Browserqualifikationen wurden nicht ausgeführt. Die laufende
UI-Snapshotqualifikation und ihre bisherigen Belege bleiben getrennt.

## 1. Belegter Ausgangspunkt

| Aktuelle Quelle | Vorhandenes Verhalten / Grenze |
|---|---|
| `crates/contracts/src/lib.rs:765,796` | `SubmitInput { request_id, answer }`; `RunDetail` enthält eine globale `input_request`. |
| `crates/runtime/src/lib.rs:346–370` | Einzelagentenfrage setzt globale Frage, Elternzustand `WaitingInput` und `lease=None`. |
| `crates/storage/src/lib.rs:1279–1313` | Antwort verlangt genaue offene ID, schreibt in `parent.checkpoint.messages`, setzt `Queued` und entfernt Lease. Wiederholte bestätigte Antwort ist derzeit kein idempotenter Beleg. |
| `crates/storage/src/workflows/tools.rs:16–89` | Broker liefert Completed/AwaitingApproval; Broker-ID bindet Lauf/Node/Turn/rohe Call-ID; nur nächster gespeicherter Vorschlag darf laufen. Inputpfad fehlt. |
| `crates/storage/src/workflows/tools.rs:528–579` | Receiptvalidierung prüft Reihenfolge, Hash, historischen Turn-Claim, Schrittfolge und exakte Reduceroperation. |
| `crates/storage/src/workflows/agents.rs:446–488` | Separate Agentenhistorien; offene Toolresultate sperren nächsten Modellrequest. Eltern-Quellhash bleibt unveränderlich. Toolresultate werden derzeit mit `result.to_string()` serialisiert. |
| `crates/domain/src/workflows/progress.rs:551,958–963` | Recovery unterbricht Running-Nodes; kein allgemeiner neuer Dispatch. |
| `crates/storage/src/workflows/runs.rs:375–380` | Allgemeines `Interrupted → Queued` ist für nicht abgeschlossene Workflows gesperrt. |
| `crates/runtime/src/workflow_coordinator.rs:80–186,234–339` | Koordinator behält Eltern-Lease/Heartbeats während Agentenfreigaben. Neue Modellanfrage erst nach allen bestätigten Receipts. |
| `crates/runtime/src/workflow_product.rs:10,176–179` | Produktwerkzeuge sind nur create_artifact, list_artifacts, read_artifact. |
| `crates/workflow-contracts/src/lib.rs:143–187` | View besitzt Nodes/Freigaben, keine Fragen oder Input-Resume-Fähigkeit. |
| `packages/ui/src/App.tsx:400–404` | Rückfrageform ausdrücklich nur für Einzelagenten. |
| `packages/ui/src/WorkflowRunPanel.tsx:11`, `workflowUi.ts:4,22` | Workflowpanel besitzt Freigaben; Editor unterstützt Artefaktwerkzeuge. |
| `crates/workflow-contracts/src/journal.rs:115–131` | Vorgänge: Create, Publish, Revoke, Start; keine Antwortoperation. Definitionsentwürfe sind keine Antwortentwürfe. |
| `services/daemon/src/lib.rs:92–113,260` | Bearer-/Origin-Prüfung, single_owner_preview; keine nachgewiesene Enterprise-Inhaltsautorisierung. |

Der Einzelagentenpfad darf nicht direkt wiederverwendet werden. Er würde die
Eltern-Lease beenden, parallele Agenten stören und den gemeinsamen Seed verändern.
Ein UI-Status allein behebt diese Grenzen nicht.

## 2. Erste vollständige Produktscheibe

- Eigene aktuelle Frage je Agent und vorgeschlagenem Werkzeugschritt;
  verschiedene Agenten dürfen gleichzeitig fragen.
- Mehrere Calls desselben Turns werden in gespeicherter Reihenfolge abgewickelt.
- Dienstseitig dauerhafte Antwortentwürfe, sichtbare Konflikte und eingefrorene
  Antwortvorgänge gehören bereits zur ersten vollständigen Produktscheibe.
- Bestätigte Antwort nur im richtigen Agentenverlauf; Elternseed und andere
  Agenten bleiben unverändert.
- Elternlauf bleibt running, normale Heartbeats und zulässige Parallelität bleiben
  erhalten. Wartende Agenten zählen weiter zum festgelegten Parallelitätslimit.
- Wiederaufnahme nur bestätigter menschlicher Wartecursor. Parallele
  unbestätigte Modellrequests oder unsichere Aktionen sperren sie.

Produktliste und Editor werden ausschließlich um request_input ergänzt.
Datei-, MCP-, Prozess- und andere externe Workflowwerkzeuge werden dadurch nicht
freigeschaltet.

## 3. Identität und Verträge

Die Frage verwendet die bestehende domänengetrennte Brokeridentität aus
run_id, node_id, turn_id und raw_tool_call_id. Ihr unveränderlicher Fingerprint
bindet zusätzlich Projekt, Workflowversion und Plan-/Definitionshash, Rolle,
vollständige kanonische Argumente, Ausführungsziel und Policyrevision.
Andere Argumente oder ein anderer Turn sind eine andere Aktion. Rohe Provider-IDs
und Rollen-IDs allein reichen nicht.

Neue DTOs sind in `crates/workflow-contracts/src/lib.rs` vorgesehen:

| DTO | Inhalt |
|---|---|
| WorkflowInputView | request_id, fingerprint, node_id, role_id, question, state, expires_at, actionable; autorisierter Antwortbeleg/Schließungsgrund |
| SubmitWorkflowInput | input: SubmitInput, fingerprint, idempotency_key |
| WorkflowInputReceipt | request_id, fingerprint, receipt_id, answer_sha256, confirmed_sequence |
| WorkflowInputDraft | request_id, fingerprint, eigene revision, answer, answer_sha256, updated_at; Eigentümer dienstseitig bestimmt |
| SaveWorkflowInputDraft | fingerprint, base_revision, idempotency_key, answer |
| WorkflowInputDraftSave | Saved oder Conflict; aktueller Kopf und erhaltene eingehende Konfliktkopie |
| PrepareWorkflowInputAnswer | fingerprint, base_draft_revision, idempotency_key |
| WorkflowInputAnswerIntent | intent_id, exakte SubmitWorkflowInput-Bindung, unveränderlicher Entwurf/hash, Zustand, Originalbeleg/Ablehnungsgrund |

WorkflowRunView erhält Fragen und gesondertes input_resume_available mit
Sperrgrund. completion_resume_available bleibt getrennt. WaitingInput am Node
bedeutet keinen globalen RunState::WaitingInput.

ApiContractV2, Schema-/OpenAPI-Generator, TypeScript und Validatoren werden gemeinsam
aus Rust erzeugt. Unbekannte Felder bleiben verboten. Inputfehler brauchen stabile
Codes; freie Error::Provider(String)-Meldungen sind dafür kein Nachweis.

## 4. Unveränderte Antworten und dauerhafte Entwürfe

Die Bytegleichheit betrifft den UTF-8-Inhalt des angenommenen SubmitInput.answer,
nicht die lexikalische JSON-Transportdarstellung. Führende/nachgestellte
Leerzeichen, Unicode, Anführungszeichen, Backslashes und LF/CRLF bleiben
unverändert. Keine Trim-, Normalisierungs- oder JSON-Stringifizierungsstufe.

Vorhandene Grenzen: Frage 4.000 UTF-8-Bytes; Antwort 32.000 Bytes, kein NUL und
nicht ausschließlich Leerraum. Leerer Entwurf ist zum Löschen/Vorbereiten
zulässig, aber noch keine Antwort. HTML-maxLength zählt nicht UTF-8-Bytes und
ersetzt keine Dienstprüfung.

Fragen/Antwortbelege liegen im bestehenden transaktionalen StoredWorkflow,
Formatversion 3; Versionen 1/2 bleiben lesbar. Autosave erhält einen eigenen
Dienstspeicher: es verbraucht keine Agentenschritte und überschreibt keine
Auftrags- oder Workflowdefinitionsentwürfe. Vorgesehen ist eine neue gemeinsame
SQLite-/PostgreSQL-Migration mit:

- workflow_input_drafts: Kopf je Lauf/Frage/serverseitigem Eigentümer,
  eigene Revision, Fingerprint, exakter Text/Hash.
- workflow_input_draft_snapshots: unveränderliche gespeicherte und konkurrierende
  Textkopien mit Basisrevision/Hash.
- workflow_input_answer_intents: eingefrorener Antwortkörper, eindeutiger
  Idempotenzschlüssel, Entwurfskopie, Pending/Accepted/Rejected und Originalbeleg.

Aktuell ist 0018.sql die letzte eingetragene Migration. Die nächste freie Nummer
wird bei Implementierung gewählt; dieses Dokument legt keine Migration an.

Eigentümeridentität kommt aus dem Dienst, niemals aus einem frei wählbaren
Browserfeld. Die erste Umsetzung bleibt Einzelbesitzer-Vorschau. Teamzugriff
erfordert später persönliche Entwurfssicht und aktuelle Inhaltsrechte.

Autosave bestätigt erst nach Commit. Konflikte erhalten beide Texte.
Absenden friert die bestätigte Entwurfsversion als unveränderlichen Intent ein.
Bei unbekanntem Ausgang wird dieser gelesen beziehungsweise exakt erneut
gecommitttet. Später bearbeiteter Text verändert keinen alten Intent.
Ein bloß gespeicherter Entwurf wird nie automatisch abgeschickt.

Nicht erreichbarer Dienst bedeutet sichtbar unbestätigte Änderungen.
React-State, Memory, BeforeUnload oder eine lokale UI-Kopie sind kein
Dauerhaftigkeitsnachweis. Allgemeine Web-Offlineausgangswarteschlangen bleiben M7.

## 5. API und geprüfte Transaktionen

Vorgesehene Routen unter /api/v2:

- Workflowansicht liefert die bestätigten Fragen.
- GET/PUT /runs/{run}/workflow/inputs/{request}/draft
- POST /runs/{run}/workflow/inputs/{request}/answer-intents
- GET /runs/{run}/workflow/input-answer-intents/{intent}
- POST /runs/{run}/workflow/input-answer-intents/{intent}/commit

Commit besitzt keinen veränderbaren Antwortkörper; der Dienst liest den
gespeicherten Intent. Intern wird SubmitWorkflowInput geprüft. Ein direkt
angebotener Antwortendpunkt darf das dauerhafte Einfrieren nicht umgehen.
Der alte /runs/{run}/input-Pfad muss Workflows ausdrücklich ablehnen.
Projekt/Eigentümerschaft stammen aus dem Lauf; ID-Kenntnis gewährt keine Rechte.

### Frage öffnen

Nur der gespeicherte nächste request_input-Vorschlag wird zugelassen.
Zuerst derselbe Policypfad: request_input ist lesend, benötigt unter ConfirmAll
aber seine konkrete Werkzeugfreigabe. Die Nutzerantwort ersetzt diese nicht.

Eine gefencete Transaktion speichert Frage, menschlichen Wartecursor, einmalige
Schrittreservierung und workflow_input_requested. Wiederholung liefert dieselbe
Frage ohne weitere Schritte/Ereignisse. Es entsteht kein externer Dispatchpermit
und keine Provider-Tokenreservierung.

### Entwurf sichern / Intent vorbereiten

Eigene Kopf-CAS-Transaktion mit exakter Frage-/Fingerprint-/Eigentümerbindung.
Sichern gewährt keine Ausführungsrechte. Ein Entwurf bleibt bei Unterbrechung
erhalten; mögliche weitere Bearbeitung darf nicht als Antwortfreigabe gelten.
Abgeschlossene Fragen sind unveränderlich, entzogene Inhaltsrechte sperren Lesen
und Schreiben. Intentvorbereitung friert nur einen tatsächlich bestätigten
Entwurf mit exakter Revision/Hash ein.

### Antwort bestätigen

Im vorhandenen Store::mutate-/Workflowwrite-Pfad:

1. Gespeicherten Intent, genaue Fragebindung und Vorschlagsplatz prüfen.
2. Aktuellen Eltern-Lease, running, Stop-on-failure und Budget prüfen.
3. Registryhead sperren; aktuelle Revocation/Policy prüfen.
4. Node-/Ahnen-/Elternfrist und typisierten Wartecursor prüfen.
5. Antwortreceipt, originalen Call, Reducerentscheidung und Accepted-Intent
   mit Originalbeleg gemeinsam speichern.
6. Run-/Workflow-CAS, Sequenzereignis und Attentionindex gemeinsam bestätigen;
   nach Locks und unmittelbar vor Commit Lease/Frist erneut prüfen.

Bestehende Fences in workflows/runs.rs::prepare, tools.rs::verify_gate und
storage/lib.rs werden um genaue Input-Gates erweitert, nicht umgangen.

Identische Wiederholung liefert denselben Beleg, ohne neue Charge, Nachricht
oder Sequenz. Andere Antwort oder gleicher Schlüssel mit anderer Bindung ist
Konflikt. Unabhängige Antworten dürfen interne CAS-Wiederholungen gegen den
aktuellen Workflowstand verwenden; das ist kein stilles Textüberschreiben.

Antwort ausschließlich im Toolreceipt dieses Agententurns.
agents::messages gibt für erfolgreiche request_input-Receipts den exakten
Antwortstring aus, nicht result.to_string(). Elternseed/hash bleiben unverändert.
workflow_tools_pending sperrt nächste Modellanfragen bis zu allen Receipts.
Ereignisse/Standardlogs enthalten IDs, Hashes und Zustände, keine Antworttexte,
Prompts oder Secrets.

## 6. Menschlicher Wartecursor und begrenzte Wiederaufnahme

Neue typisierte Reducerentscheidungen decken Öffnen, Antwort, bekannte
Schließung und begrenzte Inputfortsetzung ab. Der Cursor bindet bestätigten
Turn/Antwort-Hash, ursprünglichen Call, nächsten Callordinal sowie
waiting_answer oder answer_recorded. Die Antwortphase bleibt dauerhaft bis zur
atomaren nächsten Admission erhalten: kein ungesicherter Übergang zwischen
Antwortcommit und Modellstart.

Der Koordinator behält Eltern-Lease und Heartbeats. AwaitingInput wartet wie
die vorhandene Freigabephase, ohne globalen Zustandswechsel.
Weitere Providerrequests dieses Agenten sind währenddessen gesperrt.

Bei Leaseablauf wird der Elternlauf wie heute interrupted. Der typisierte
menschliche Cursor bleibt prüfbar; normale Running-Nodes und unsichere Wirkungen
werden weiterhin unterbrochen. Ausdrückliches Input-Resume prüft:

- Alle wiederaufzunehmenden aktiven Nodes besitzen gültige menschliche
  Wartecursor mit bestätigter Modellantwort.
- Kein paralleler unbestätigter Modellrequest und keine unsichere externe
  Aktion existieren. Solche Fälle sperren die gemeinsame Fortsetzung.
- Keine Revocation/Cancel/Ablauf, gültige Plan-/Historienhashes und zulässiger
  Arbeitsverzeichnisbesitz.
- Pending-Nodes ohne Admission und abgeschlossene Nodes bleiben unverändert;
  beliebige Running-/Approval-Claims werden nicht übernommen.

Zulässiges Resume stellt den Lauf in die normale Queue. Nur diese vergibt
eine neue Eltern-Lease. Der Koordinator liest den bestätigten Turn als
lokalen Fortsetzungsbeleg und sendet dessen Modellrequest nicht erneut.
Gespeicherte Receipts werden nur gelesen. Restliche vorgeschlagene Calls dürfen
in diesem Schnitt ausschließlich transaktionale Artefaktwerkzeuge oder Input sein.

Historische Turns/Claims werden nicht umgeschrieben. Ein expliziter
Cursorübergabebeleg bindet ihre begrenzte Abwicklung unter der neuen Eltern-Lease.
Für die nächste Modellanfrage wird neue aktive Claim-/Admissionzuständigkeit
atomar erzeugt. agents::active, Reducer und Receiptvalidierung müssen die
Übergabe explizit nachvollziehen; historische Claims werden nicht pauschal
wieder als aktive Ausführungsberechtigung akzeptiert.

Alte Worker können ihre alte Eltern-Lease nicht nutzen. Eine gespeicherte
unbestätigte Modelladmission wird niemals erneut gesendet.
Allgemeines Resume/Reconciliation für gemischte oder unsichere Fälle bleibt offen.

## 7. Fristen, Entzug und Abbruch

Wartezeit verlängert kein Budget. expires_at ist das Minimum der Eltern-,
Node- und Ahnenfrist; actionable berücksichtigt die aktuelle Lease.
Leaseablauf ist Unterbrechung, keine Verlängerung der Fragefrist.
Die UI-Uhr entscheidet keine Rechte.

Revocation, CancelRequested, Fristablauf und Stop-on-failure sperren neue
Antworten und schließen Fragen nachvollziehbar. Menschliches Warten ist keine
unbekannte externe Wirkung; seine bekannte Schließung darf archiviert werden,
ohne einen unbestätigten Providerrequest als erledigt auszugeben.
Schrittreservierung wird durch Neustart/erneutes Öffnen nicht zurückgesetzt.

Bestätigte identische Antwortbelege dürfen mit weiterhin gültigen Leserechten
nach Laufabschluss gelesen werden. Das erteilt keine neue Ausführungsbefugnis.

## 8. Betroffene Implementierungsstellen

| Bereich | Vorgesehene Änderung |
|---|---|
| crates/domain/src/workflows/progress.rs | Inputcursor/Entscheidungen, Charge, Replayvalidierung, enge Continuation |
| crates/domain/src/lib.rs | Stabile Input-/Draftfehler, soweit vorhandene Codes nicht ausreichen |
| crates/storage/src/workflows.rs | Inputmodul, Format 3, alte Formate |
| Neues crates/storage/src/workflows/inputs.rs | Frage-/Antwortdienst, genaue Bindungen/Gates und Belege |
| Neues crates/storage/src/workflow_input_drafts.rs | Dienstseitige Entwürfe, Konfliktkopien, eingefrorene Intents |
| crates/storage/src/migrations.rs, neue Migration | Entwurf-/Snapshot-/Intenttabellen für beide Adapter |
| storage/workflows/tools.rs, agents.rs, runs.rs; storage/src/lib.rs | AwaitingInput, exakte Textnachricht, Receipt-/Claimübergabe, Commitfences, enge Recovery, Workflowablehnung am alten Inputpfad |
| crates/storage/src/workflows/product.rs | Fragen/Resume-Grund, bestehender Attentionindex zählt Freigaben und Fragen |
| runtime/src/workflow_product.rs, workflow_coordinator.rs, ggf. workflow_models.rs | Katalog, Warten, sichere Cursorübernahme, atomare nächste Admission |
| workflow-contracts/src/lib.rs; tools/contracts/src/main.rs | DTOs, Schemas/OpenAPI, gemeinsame Generierung |
| services/daemon/src/workflow_routes.rs, lib.rs | Autorisierte Draft-/Intent-/Commitpfade und Altpfadsperre |
| packages/client/src/index.ts, generierte Typen/Validatoren | Neue APIoperationen, keine zweite handgepflegte Vertragsquelle |
| packages/ui/src/WorkflowRunPanel.tsx, App.tsx, workflowUi.ts, Übersetzungen | Mehrere Fragen, Speichern/Konflikte, eingefrorene Antwort, verständliches enges Resume |

UI zeigt Rolle, Schrittziel, Frage, Frist und Speicherzustand. IDs bleiben
Diagnosen. Felder bleiben je Request stabil; Polling ersetzt keinen Text.
Offene Intents werden nach erneutem Öffnen aus dem Dienst gelesen.
Konflikte/unbekannte Bestätigungen sind sichtbar. Dienstzustand entscheidet
Aktionen, niemals ein Reactstatus oder clientseitiger Timer.

## 9. Verpflichtende Akzeptanzfälle

Tests führen tatsächliche Dienst-, Runtime- und HTTPpfade aus.
Quelltext-/ASTprüfungen sind ausschließlich Strukturbelege.

| Bereich | Notwendiger Nachweis |
|---|---|
| Bindung | Gleiche rohe Call-ID zweier Agenten bleibt getrennt; fremder Lauf/Projekt/Turn/Fingerprint/Vorschlagsplatz wird abgelehnt |
| Antwortbytes | Leerzeichen, Unicode, LF/CRLF, Quotes, Backslashes exakt beim richtigen nächsten Modellrequest; Elternseed/andere Agenten unverändert |
| Entwurf | Bestätigtes Autosave überlebt UI-/Daemonneustart; DB-/Datenträgerfehler ergeben keine falsche Speicherbestätigung |
| Draftkonflikt | Zwei Fenster derselben Basis erhalten beide Texte, kein stilles Überschreiben |
| Intent | HTTPantwortverlust/Neustart/erneutes Commit verwenden Originalintent/text; neuer Entwurf verändert alten Vorgang nicht |
| Deduplikation | Identische Commits: genau ein Beleg/Receipt/Schritt/Ereignis; andere Antwort oder Schlüsselbindung: Konflikt |
| Parallelität | Zwei Fragen gleichzeitig beantwortbar; weitere zulässige Arbeit/Heartbeats bleiben aktiv; MaxParallel eingehalten |
| Policy | Alle vier Modi, echter ConfirmAll-Freigabeablauf; Antwort erweitert keine Rechte |
| Fences | Revocation/Cancel/Frist/Leaseverlust vor und während Registrylock sowie vor Commit verhindern Antwortwirkung |
| Neustart | Abbruch vor/nach Frage-/Antwortcommit; bestätigter Cursorresume mit neuer Lease, kein erneuter alter Providerrequest |
| Negatives Resume | Paralleler unbestätigter Modellrequest/unsicherer Toolausgang sperrt Resume und neues IO |
| Alte Worker | Alte Lease verändert keine Fragen/Antworten und lässt keine neue Modelladmission zu |
| Callfolge/Budgets | Mehrere Calls/Fragen, Erschöpfung, bekannte Schließung; kein Modellstart vor vollständigen Receipts |
| Speicher | SQLite/PostgreSQL gleiche Fälle, Transaktionsabbruch, erneutes Öffnen, Migrationschecksum, Format-1/2-Lesbarkeit |
| API/UI | Tatsächliche Autorisierung, unbekannte Felder/IDs, mehrere Formulare, Tastatur/Fokus, 200 % Zoom, lange Inhalte, DE/EN und sichtbare Speicher-/Konfliktzustände |

Tatsächliche Provider-/Toolzähler und Modellrequests belegen, dass abgewiesene
oder unklare Wiederaufnahmen kein IO auslösen. Fingerprint-Unitprüfungen allein
beweisen das nicht. Neue Frontend-/Browser-/Desktopqualifikationen bekommen
eigene Quellenbelege; frühere Qualifikationen werden nicht nachträglich erweitert.

## 10. Reihenfolge und Abschlussgrenze

1. Verträge, Speicher-/Entwurfsdienste, Reducer und echte Transaktionstests.
2. Broker/Koordinator, begrenzte Recovery und negative Worker-/Providerfälle.
3. Vollständiges Antwortprodukt mit dauerhaften Entwürfen, Konflikten und Intents.
4. Eigenständige Adapter-/HTTP-/Browser-/Windows-/Linux-Lebenszyklusnachweise
   und aktualisierte Betriebsbeschreibung.

Produktquellen werden erst nach Ende der laufenden Snapshot-Browserfixture und
separatem Implementierungsauftrag geändert. Eine UI-/Memorylösung gilt nicht
als vollständige Produktscheibe.

Bis tatsächliche Implementierung und bestandene Fälle vorliegen, bleibt
Workflow-Input **nicht implementiert / nicht abgenommen**. Auch danach bleiben
allgemeines Resume, Enterprise/Offline und vollständiges M0–M10 gesondert offen.
