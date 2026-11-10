import workflowMessages from "./workflowMessages.json";
import secretMessages from "./secretMessages.json";
import mcpMessages from "./mcpMessages.json";
import moneyMessages from "./moneyMessages.json";
import connectionMessages from "./connectionMessages.json";
import structuredOutputMessages from "./structuredOutputMessages.json";
export const de = {
  regeneration_workflow_input_transport_required:"Diese Workflow-Eingaben lassen sich im Browser nicht verlustfrei für einen neuen Auftrag übernehmen. Der ursprüngliche Lauf bleibt erhalten; es wurde kein weiterer Lauf gestartet.",
  regeneration_workflow_workspace_binding_required:"Dieser Workflow bindet Dateiordner an den ursprünglichen Chat. Für einen neuen Chat ist eine ausdrücklich passende Ordnerbindung erforderlich. Es wurde kein weiterer Lauf gestartet.",
  regeneration_requires_model_answer:"Für diese Ausführungsart ist noch keine bestätigte Regeneration verfügbar. Verwende den Prompt im passenden Auftrag erneut.",
  workflow_input_source_mismatch:"Die ursprünglichen Workflow-Eingaben stimmen nicht mit dem gespeicherten Beleg überein. Es wurde kein Ersatzauftrag gestartet.",
  workflow_input_source_requires_plan_v2:"Die ursprünglichen Workflow-Eingaben benötigen die bestätigte Ausführungsprüfung.",
  runCopyRestoreHint:"Die Wiederherstellung erhält einen bestätigten Plan und eine neue Arbeitskopie. Originaldateien und vorhandene Ergebnisversionen bleiben unverändert.",
  runCopyRestoreUnavailable:"Diesen Dateistand kannst du herunterladen oder über „Ab hier verzweigen“ in einem eigenen Chat weiterverwenden. Die Wiederherstellung einzelner Dateien ist hier noch nicht verfügbar.",
  waiting_for_timer:"Geplante Fortsetzung",
  workflowWaitingTimer:"Wartet bis zum Termin",
  workflowNodeSkipped:"Übersprungen",
  saved_chat_provider_revision_unavailable:"Die gespeicherte Modellrevision dieses Chats ist nicht mehr verfügbar. Wähle ausdrücklich ein aktuelles Modellprofil.",
  chat_configuration_not_saved:"Die Chatkonfiguration ist noch nicht bestätigt. Warte auf die Speicherung oder kläre die ursprüngliche Anfrage erneut.",
  library_source_unavailable:"Die ursprüngliche Bibliotheksvorlage ist nicht mehr verfügbar. Die gespeicherte Kopie bleibt erhalten.",
  chat_configuration_save_busy:"Eine Speicherung der Chatkonfiguration läuft bereits. Warte auf deren Bestätigung.",
  chat_configuration_unavailable:"Die Chatkonfiguration konnte nicht geladen werden. Prüfe die Verbindung und lade die Serverkopie erneut.",
  chat_configuration_receipt_mismatch:"Die Speicherbestätigung passt nicht zur ursprünglichen Chatkonfiguration. Die Anfrage bleibt für den Abgleich erhalten.",
  chat_configuration_draft_conflict:"Dieser Chat wurde inzwischen in einer anderen Ansicht konfiguriert. Lade die Serverkopie oder speichere deinen sichtbaren Konfigurationsentwurf ausdrücklich.",
  chat_configuration_invalid:"Die Chatkonfiguration enthält ungültige Werte. Prüfe den Entwurf vor dem Speichern.",
  chat_configuration_credentials_forbidden:"Zugangsdaten dürfen nicht im lokalen Konfigurationsentwurf gespeichert werden. Verwende die Anbieter- oder Secret-Einstellungen.",
  request_journal_invalid:"Die lokale Speicherreferenz ist beschädigt. Verwirf sie ausdrücklich und lade die gespeicherte Konfiguration erneut.",
  request_journal_changed:"Eine andere Ansicht hat eine offene Speicheranfrage hinterlegt. Es wurde keine Anfrage überschrieben. Kläre zuerst den gespeicherten Stand.",
  request_journal_unavailable:"Der lokale Speicher für die Wiederaufnahme ist nicht verfügbar. Die Konfiguration konnte nicht sicher vorbereitet werden.",
  default_provider_unavailable:"Das gespeicherte Standardmodell ist nicht mehr verfügbar. Wähle ein gültiges Profil in Teams & Tools.",
  default_agent_unavailable:"Die gespeicherte Standardvorlage ist nicht mehr verfügbar. Wähle einen Agent in den Einstellungen.",
  selected_skill_unavailable:"Ein ausgewählter Skill wurde gelöscht oder deaktiviert. Aktualisiere die Agentvorlage.",
  selected_memory_unavailable:"Ein ausgewählter Wissenseintrag ist nicht mehr verfügbar. Aktualisiere die Agentvorlage.",
  selected_mcp_tool_unavailable:"Ein ausgewähltes MCP-Werkzeug hat sich geändert oder ist nicht verfügbar. Aktualisiere die Agentvorlage.",
  ...structuredOutputMessages.de,
  ...moneyMessages.de,
  ...connectionMessages.de,
  ...secretMessages.de,
  ...mcpMessages.de,
  ...workflowMessages.de,
  budget_configuration:"Das Tokenbudget muss mindestens ein Kontextfenster des gewählten Modells abdecken. Die Antwortgrenze muss kleiner als dieses Fenster sein.",context_limit:"Die Unterhaltung samt Werkzeugdefinitionen passt nicht in die aktuelle Kontextgrenze. Wähle ein passendes Modellprofil oder beginne einen neuen Chat mit gezielt ausgewähltem Kontext.",reservationPerRequest:"Reservierung je Modellanfrage",
  runBudgets:"Grenzen für den nächsten Lauf",runBudgetStatus:"Tokenbudget dieses Laufs",maxRunSteps:"Maximale Agentenschritte",maxRunSeconds:"Maximale Laufzeit (Sekunden)",maxReplyTokens:"Maximale Token je Modellantwort",maxRunTokens:"Tokenbudget für den gesamten Lauf",budgetHint:"Diese Grenzen werden beim Start eingefroren. Laufzeit umfasst auch Freigaben und Rückfragen.",tokenReservationHint:"Jede Modellanfrage reserviert zunächst ein vollständiges konfiguriertes Kontextfenster. Bestätigte Verbrauchswerte ersetzen die Reservierung; fehlende Angaben bleiben gebunden. Ohne ausreichenden Rest startet keine weitere Anfrage.",remainingTokenBudget:"Rechnerisches Restbudget",unconfirmedTokenBudget:"Davon vorsorglich gebunden",usageIncomplete:"Verbrauchsangaben sind unvollständig oder stehen noch aus. Die angezeigten gemeldeten Token sind keine vollständige Abrechnung; gebundene Reservierungen bleiben erhalten.",providerBudgetOverrun:"Der Anbieter hat eine Grenze überschritten. Weitere Modell- und Werkzeugaktionen dieses Laufs sind gesperrt.",

  manageFolderAccess:"Ordnerfreigaben verwalten",nativeFolderHint:"Wähle im Desktop einen vorhandenen Ordner und bestätige die Rechte. Neue Freigaben erlauben zunächst nur Lesen. Die Auswahl für einen Lauf erfolgt oben getrennt.",grantFolderChanges:"Für den neuen Ordner auch Änderungen zulassen",chooseFolder:"Ordner auswählen",refreshFolderAccess:"Freigaben aktualisieren",retryFolderChoice:"Ordnerauswahl abgleichen",folderChoicePending:"Die vorige Anfrage bleibt erhalten. Beim erneuten Aufruf wird zuerst geprüft, ob ihre Freigabe bereits gespeichert wurde.",folderChoiceCanceled:"Ordnerauswahl abgebrochen.",folderRegistered:"Ordner freigegeben. Wähle ihn oben ausdrücklich für den nächsten Lauf aus.",folderRevoked:"Ordnerzugriff entzogen. Die Dateien bleiben erhalten.",folderAccessRevoked:"Zugriff entzogen",revokeFolderAccess:"Zugriff entziehen",confirmFolderRevocation:"Entzug bestätigen",folderRevokeHint:"Nach Bestätigung darf kein neuer Dateizugriff über diese Freigabe beginnen. Laufende native Dateioperationen müssen zuerst abschließen. Unklare frühere Änderungen bleiben abgleichbar; die Dateien werden nicht gelöscht.",folderRevokeBusy:"Der Ordner wird gerade verwendet oder seine Freigabe wird bearbeitet. Warte auf den Abschluss und bestätige den Entzug erneut.",folder_desktop_required:"Zusätzliche Rechnerordner richtest du im verbundenen Desktop ein. Der Browser kann nur bereits erteilte Freigaben für einen Lauf auswählen.",folder_connection_failed:"Die Ordnerverwaltung ist nicht erreichbar. Prüfe die Desktop-Verbindung und aktualisiere die Freigaben.",folder_picker_busy:"Eine native Ordnerauswahl ist bereits geöffnet. Schließe sie zuerst ab.",folder_registration_uncertain:"Die Ordneränderung ist noch nicht bestätigt. Aktualisiere die Freigaben oder gleiche dieselbe Anfrage erneut ab.",
  refreshUploads:"Übertragungen aktualisieren",addFile:"Datei hinzufügen",uploadHint:"Wähle eine Datei bis 64 MiB. Die Übertragung speichert eine geprüfte Kopie für diesen Chat. Den Import in eine neue Arbeitskopie prüfst und bestätigst du danach.",chooseLocalFile:"Datei auswählen",uploadFile:"Datei übertragen",uploadBusy:"Datei wird geprüft und übertragen …",uploadReadyHint:"Datei geprüft gespeichert. Wähle „Datei importieren“, um eine neue Arbeitskopie vorzubereiten.",uploadReady:"Übertragung bestätigt",uploadReserved:"Übertragung ausstehend",uploadContinue:"Übertragung fortsetzen",uploadedFiles:"Übertragene Dateien",clearSelection:"Auswahl zurücksetzen",uploadFailed:"Die Übertragung ist nicht bestätigt. Aktualisiere die Liste oder wiederhole sie mit derselben Datei.",uploadMismatch:"Die ausgewählte Datei passt nicht zur offenen Übertragung. Wähle die ursprüngliche Datei oder setze die Auswahl zurück.",upload_too_large:"Diese Datei überschreitet die Grenze von 64 MiB.",upload_integrity:"Datei und Übertragungsnachweis stimmen nicht überein. Die Arbeitsdateien wurden nicht automatisch geändert.",transfer_busy:"Es laufen bereits zwei Dateiübertragungen. Warte auf deren Abschluss und versuche es erneut.",
  import_file:"Datei importieren",document_read:"Dokument lesen",document_write:"Dokument erstellen oder ändern",image_edit:"Bild bearbeiten",generatedFile:"Neue Datei erzeugt",generatedBy:"Erzeugt mit",documentOperation:"Vorgang",documentInputs:"Quelldateien",documentOutput:"Neue Datei",documentContent:"Inhalt",documentNoOverwrite:"Es wird eine neue Datei angelegt. Bestehende Dateien werden nie überschrieben.",imageSteps:"Bearbeitungsschritte",documentPreviews:"Vorschau",previewUnavailable:"Vorschau nicht verfügbar",importHint:"Ein bestätigter Lauf importiert die geprüfte Datei in eine neue Arbeitskopie. Originale und vorherige Ergebnisse bleiben erhalten. Es wird kein Modell aufgerufen.",importReadOnly:"Im Modus „Nur lesen“ ist kein Dateiimport möglich.",checkDestination:"Ziel prüfen",destinationExists:"An diesem Ziel ist bereits ein Eintrag vorhanden.",destinationNew:"An diesem Ziel wird eine neue Datei angelegt.",destinationDirectory:"Das Ziel ist ein Verzeichnis. Wähle einen Dateipfad.",confirmReplace:"Die geprüfte Version in der neuen Kopie ersetzen. Die Originaldatei bleibt erhalten.",startImport:"Import prüfen",importPending:"Die Startanfrage ist noch nicht bestätigt. Eine Wiederholung fragt denselben Lauf an; Ziel und Zugriffsmodus bleiben eingefroren.",uploadedSource:"Übertragene Quelle",
  fileSaved:"Datei gespeichert und geprüft.",saveCanceled:"Speichern abgebrochen.",save_busy:"Ein Speicherdialog ist bereits geöffnet. Schließe ihn zuerst ab.",save_forbidden:"Dieses Ziel ist nicht zulässig. Wähle eine reguläre Datei außerhalb der privaten Anwendungsdaten.",save_conflict:"Die Zieldatei oder ihr Ordner hat sich geändert. Prüfe den Stand und wähle das Ziel erneut.",save_invalid:"Die Datei oder ihre Prüfdaten sind ungültig. Lade den ursprünglichen Inhalt erneut.",save_failed:"Die Datei konnte nicht gespeichert werden. Prüfe Speicherplatz und Zugriffsrechte.",save_uncertain:"Der Abschluss der Dateiablage ist unbestätigt. Prüfe zuerst die gewählte Zieldatei, bevor du erneut speicherst.",
  downloadCurrent:"Aktuelle Datei herunterladen",downloadSnapshot:"Gesicherten Dateistand herunterladen",downloadChecking:"Datei wird geladen und geprüft …",downloadPrepared:"Download vorbereitet. Prüfe die Dateiablage deines Browsers.",downloadFailed:"Der Download konnte nicht sicher vorbereitet werden. Prüfe die Verbindung und die Dateirechte und versuche es erneut.",downloadChanged:"Die Datei wurde seit der Vorschau geändert. Aktualisiere die Vorschau vor dem Download.",downloadBusy:"Der Dateibestand wird gerade geändert. Versuche den Download nach Abschluss erneut.",downloadLimit:"Downloads sind auf 64 MiB pro Datei begrenzt.",workspacePreviewUnavailable:"Für diese Datei ist keine Textvorschau verfügbar. Der Download wird separat geprüft.",
  restore_file:"Dateistand wiederherstellen",nativeFileAction:"Dateiaktion ohne Modell",restoreHint:"Ein aufbewahrter Dateistand wird als eigene Kopie im selben freigegebenen Ordner wiederhergestellt. Der Zielpfad muss frei sein und sein übergeordnetes Verzeichnis existieren.",restoreApprovalHint:"Dies erstellt einen eigenen Lauf. Bestätige anschließend die genaue Aktion. Der aktuelle Entwurf bleibt erhalten; ein Modell wird nicht benötigt.",prepareRestore:"Wiederherstellung vorbereiten",restorePrevious:"Vorherige Version wiederherstellen",restoreWritten:"Geschriebene Version wiederherstellen",
  stat_file:"Dateistand prüfen",create_directory:"Verzeichnis anlegen",rename_file:"Datei oder Verzeichnis verschieben",remove_file:"Datei oder Verzeichnis entfernen",destinationPath:"Zielpfad",removeFileHint:"Der sichtbare Eintrag wird entfernt. Der geprüfte Inhalt bleibt für die Wiederherstellung aufbewahrt.",retainedFiles:"Aufbewahrte Einträge",


  workingListTruncated:"Es werden die ersten 500 Einträge angezeigt.",

  workingFiles:"Arbeitsdateien",workingFilesHint:"Der aktuelle Dateibestand dieses Chats bleibt für weitere Läufe erhalten. Der gewählte Zugriffsmodus gilt auch hier.",

  workingFilesEmpty:"Noch keine Arbeitsdateien. Das Modell kann hier Dateien anlegen, wenn der gewählte Zugriffsmodus es erlaubt.",

  workingFilesBusy:"Die Arbeitsdateien werden gerade verwendet. Versuche es nach Abschluss des Vorgangs erneut.",workingFilesUnavailable:"Die Arbeitsdateien konnten nicht gelesen werden.",folderUp:"Übergeordneter Ordner",refreshFiles:"Dateien aktualisieren",blockedFile:"Nicht zugänglich",

  workspaceSnapshot:"Gesicherter Dateistand dieses Laufs",workspaceSnapshotHint:"Dieser Stand gehört zum gewählten Ergebnis. Verzweigungen erhalten daraus eine eigene Arbeitskopie.",workspaceInherited:"Arbeitskopie aus dem gewählten historischen Dateistand",

  workspaceTextHint:"Vorschau für UTF-8-Text bis 1 MB. Binärdateien können derzeit nicht angezeigt werden.",



  filePath:"Datei im freigegebenen Ordner",

  folderContext:"Ordnerkontext",noFolderAccess:"Keine zusätzlichen Ordner ausgewählt",folderHint:"Wähle die ausdrücklich freigegebenen Ordner für den nächsten Lauf. Der Zugriffsmodus gilt zusätzlich. Inhalte können an das ausgewählte Modell übertragen werden.",

  noConfiguredFolders:"Für dieses Projekt ist noch kein zusätzlicher Ordner freigegeben.",

  folderWritable:"Lesen und freigegebene Änderungen",folderReadOnly:"Ausschließlich lesen",folderGrantChanged:"Eine gewählte Ordnerfreigabe wurde geändert oder entfernt. Entferne die Auswahl und prüfe die neuen Rechte vor dem Start.",clearFolders:"Auswahl entfernen",

  fileChanges:"Dateiänderungen dieses Laufs",compareVersions:"Versionen vergleichen",previousVersion:"Vorherige Version",proposedVersion:"Vorgeschlagene Version",newFile:"Neue Datei",versionUnavailable:"Die gesicherte Version konnte nicht gelesen werden.",

  uncertainWrite:"Ausgang der Dateiänderung klären",uncertainHint:"Die Änderung wurde begonnen, ihr Abschluss ist noch nicht bestätigt. Vor einer Fortsetzung muss der tatsächliche Dateistand abgeglichen werden.",

  reconcileHint:"Der Daemon prüft Inhalt, Identität und Pfade. Die Entscheidung wird nur übernommen, wenn der tatsächliche Dateistand exakt zur gewählten Möglichkeit passt. Danach bleibt der Lauf bis zu deiner Fortsetzung unterbrochen.",

  confirmNotApplied:"Unveränderten Ausgangsstand bestätigen",acceptCompleted:"Änderung als ausgeführt übernehmen",writeInProgress:"Der Abschluss dieser Änderung wird geprüft.",

  list_files:"Dateien auflisten",read_text_file:"Textdatei lesen",write_text_file:"Textdatei schreiben",workspace_busy:"Ein anderer Lauf besitzt diesen Ordner. Der Schreibzugriff wartet auf seine Freigabe.",file_conflict:"Die Datei entspricht nicht der erwarteten Version. Prüfe den aktuellen Stand vor weiteren Änderungen.",uncertain_effect:"Die letzte Änderung hat einen unbestätigten Ausgang. Kläre zuerst den Dateistand.",executor_interrupted:"Die Ausführung wurde unterbrochen. Prüfe den bisherigen Verlauf vor der Fortsetzung.",

  activity:"Aktivität",inbox:"Eingang",allActive:"Alle offenen Läufe",activityHint:"Freigaben, Rückfragen und laufende Arbeit aus allen lokalen Projekten.",

  inboxEmpty:"Keine offenen Freigaben oder Rückfragen. Auch unterbrochene Läufe erscheinen hier.",activityEmpty:"Derzeit keine offenen Läufe.",

  searchHistory:"Verläufe durchsuchen",searchHistoryHint:"Suche in Chatstiteln, Nachrichten und Ergebnissen.",searchScope:"Suchbereich",allProjects:"Alle Projekte",currentProject:"Aktuelles Projekt",noSearchResults:"Keine passenden Inhalte gefunden.",

  previousPage:"Zurück",nextPage:"Weiter",exportOrder:"Chat exportieren",exportHint:"Lade den bestätigten Stand mit Unterhaltung, Ergebnissen, Laufverlauf und Freigaben als JSON-Datei herunter. Der Export enthält einen Integritätshash und ist kein ausführbares Migrationspaket.",downloadExport:"JSON herunterladen",

  branch:"Zweig",branchHere:"Ab hier verzweigen",branchHint:"Ein eigener Chat übernimmt den unveränderten Verlauf bis zu dieser Antwort und die dazugehörigen Ergebnisse. Ein vorhandener Dateisnapshot wird als getrennte Arbeitskopie übernommen; externe Ordner bleiben unverändert. Neue Fortsetzungen bleiben getrennt. Laufende Aktionen und Freigaben werden nicht übernommen.",branchBoundary:"Gewählte letzte Antwort",createBranch:"Zweig erstellen",inheritedHistory:"Übernommener Verlauf",inheritedHint:"Dieser historische Stand bleibt unverändert. Neue Nachrichten gehören zu diesem Chat.",openOrigin:"Ursprung öffnen",historical:"Historisch",

  app: "LocalAI Cowork", preview: "V2 · Entwicklung", workspace: "Arbeitsplatz", projects: "Projekte", orders: "Chats",

  newProject: "Neues Projekt", newOrder: "Neuer Chat", projectName: "Projektname", orderName: "Titel des Chats",

  create: "Erstellen", close: "Schließen", cancel: "Abbrechen", save: "Speichern", settings: "Einstellungen",

  connect: "Arbeitsplatz verbinden", connectText: "Verbinde deinen lokalen Arbeitsplatz oder deinen eigenen V2-Server. Projekte und Chats bleiben auf dem gewählten System gespeichert.",

  token: "Verbindungsschlüssel", tokenHelp: "Den privaten Schlüssel findest du im V2-Datenordner in daemon.token. Er wird nur für diese Browsersitzung gespeichert.",

  connectAction: "Verbinden", disconnect: "Verbindung trennen", local: "Dieser Rechner", synced: "Verbindung steht", connecting: "Verbindung wird hergestellt",

  disconnected: "Keine Verbindung", reconnect: "Erneut verbinden", serverUnavailable: "Der Daemon ist nicht erreichbar. Deine bestätigten Daten bleiben gespeichert.",

  hello: "Was möchtest du erledigen?", intro: "Ein Ziel, ein Chat, ein nachvollziehbares Ergebnis.",

  startProject: "Lege ein Projekt an", emptyProject: "Ein gemeinsamer Ort für deine Chats und Ergebnisse. Dieses Projekt bleibt lokal.",

  startOrder: "Chat beschreiben", emptyOrders: "Noch keine Chats", emptyMessages: "Beschreibe dein Ziel und woran du ein gutes Ergebnis erkennst.",

  prompt: "Was soll erledigt werden?", promptPlaceholder: "Zum Beispiel: Erstelle eine verständliche Zusammenfassung …",

  send: "Lauf starten", sending: "Wird gestartet …", saved: "Entwurf gespeichert", unsaved: "Noch nicht gespeichert", saving: "Entwurf wird gespeichert …",

  draftConflict: "Der Chat wurde inzwischen geändert. Dein Entwurf bleibt hier erhalten. Prüfe den gespeicherten Stand, bevor du ihn ersetzt.",

  useServerDraft: "Gespeicherten Entwurf laden", keepMyDraft: "Meinen Entwurf nach Prüfung speichern",

  noProvider: "Zuerst ein Modell einrichten", chooseProvider: "Modell", providerSetup: "Modell verbinden", providerHint: "Trage ein vorhandenes Modell ein. Es werden keine Modelle heruntergeladen.",

  providerName: "Name", providerType: "Anbieter", endpoint: "Endpunkt", model: "Modellname", context: "Kontextgrenze (Token)",

  credentialHint: "Anbieter mit API-Schlüssel werden über providers.json und eine native Umgebungsvariable verbunden. Schlüssel werden nicht im Browser gespeichert.",

  remoteHint: "Bei Auswahl eines entfernten Endpunkts werden Chatsinhalte an diesen Anbieter gesendet.",

  access: "Zugriffsmodus", read_only: "Nur lesen", confirm_mutations: "Änderungen bestätigen", confirm_all: "Jede Aktion bestätigen", autonomous: "Innerhalb der Freigabe autonom",

  scope: "Freigegebene Werkzeuge: Ergebnisse lesen und Textartefakte erstellen.",

  results: "Ergebnisse", conversation: "Unterhaltung", details: "Details", noResults: "Ergebnisse erscheinen hier, sobald ein Lauf sie erstellt.",

  runs: "Ausführungen", noRuns: "Noch keine Ausführung", currentRun: "Ausführung", you: "Du", assistant: "Cowork",

  queued: "In Warteschlange", waiting_for_executor: "Wartet auf Ausführung", waiting_for_snapshot: "Kontext wird vorbereitet",

  running: "In Arbeit", waiting_approval: "Freigabe erforderlich", waiting_input: "Rückfrage offen", cancel_requested: "Abbruch angefordert",

  interrupted: "Unterbrochen", completed: "Abgeschlossen", failed: "Fehlgeschlagen", canceled: "Abgebrochen", expired: "Budget erreicht",

  stopRun: "Lauf abbrechen", resume: "Kontrolliert fortsetzen", retry: "Erneut laden", approvalTitle: "Deine Freigabe ist erforderlich",

  approvalDescription: "Die Entscheidung gilt genau für diese Aktion und ihre angezeigten Argumente.",

  approve: "Aktion erlauben", deny: "Aktion verweigern", question: "Rückfrage", answer: "Deine Antwort", submitAnswer: "Antwort senden",

  create_artifact: "Ergebnis erstellen", list_artifacts: "Ergebnisse auflisten", read_artifact: "Ergebnis lesen", request_input: "Rückfrage stellen",

  diagnostics: "Technische Details", download: "Herunterladen", artifactIntegrity: "SHA-256 und Herkunft", events: "Gespeicherte Ereignisse",

  cost: "Kosten", unknown: "Unbekannt", tokens: "Token (Eingabe / Ausgabe)", steps: "Agentenschritte",

  search: "Chats suchen", openNavigation: "Navigation öffnen", toggleResults: "Ergebnisse einblenden", toggleTheme: "Darstellung wechseln",

  light: "Hell", dark: "Dunkel", language: "Sprache", loading: "Wird geladen …", error: "Aktion konnte nicht abgeschlossen werden.",
  policy_denied: "Die Freigabe wurde verweigert oder eine geltende Richtlinie verhindert diese Aktion.",

  revision_conflict: "Der Stand wurde gleichzeitig geändert. Bitte neu laden und Änderungen prüfen.",

  authentication_required: "Der Verbindungsschlüssel ist ungültig.", forbidden: "Diese Aktion ist nicht freigegeben.",

  invalid_request: "Bitte überprüfe deine Eingaben.", provider_unavailable: "Das ausgewählte Modell ist nicht verfügbar. Prüfe Modellname, Endpunkt und Zugangsdaten.",
  provider_request_rejected: "Der Modellserver hat die Anfrage abgelehnt (HTTP 400/422). Prüfe die unterstützten Anfrageparameter und das Chatformat.",
  provider_authentication_failed: "Der Modellserver hat den API-Schlüssel abgelehnt (HTTP 401).",
  provider_access_denied: "Der Modellserver verweigert den Zugriff (HTTP 403).",
  provider_endpoint_not_found: "Der API-Pfad wurde nicht gefunden (HTTP 404). Prüfe den Endpunkt einschließlich /v1.",
  provider_rate_limited: "Der Modellserver begrenzt gerade die Anfragen (HTTP 429).",
  provider_connection_timeout: "Der Modellserver hat nicht innerhalb des Zeitlimits geantwortet.",
  provider_tls_failed: "Die verschlüsselte Verbindung zum Modellserver konnte nicht hergestellt werden. Prüfe das Serverzertifikat.",
  provider_connection_failed: "Die Verbindung zum Modellserver ist fehlgeschlagen.",

  storage_unavailable: "Speichern ist gerade nicht möglich. Unbestätigte Änderungen bleiben als Entwurf sichtbar.",

  stale_lease: "Diese Ausführung wird nicht mehr von diesem Worker betreut.",

  budget_exceeded: "Das Ausführungsbudget ist aufgebraucht. Ein neuer Lauf benötigt eine neue Konfiguration.",

  previewNote: "Lokale Entwicklungsversion · keine Enterprise-Freigabe",

  emptyProvider: "Noch kein Modell verbunden", localOnly: "Privat · lokal", pending: "Offen", selectRun: "Ausführung auswählen",

  begin: "Dein Arbeitsplatz ist bereit", navigation: "Projekte und Chats", stream: "Aktuelle Antwort", returnConversation: "Zur Unterhaltung",

  confirmDraft: "Gespeicherter Entwurf", problemDetails: "Fehlerdetails", selected: "Ausgewählt", runKeepsGoing: "Angenommene Läufe laufen auch bei geschlossenem Fenster weiter.",

} as const;

export type Key=keyof typeof de;

export const en:Record<Key,string> = {
  regeneration_workflow_input_transport_required:"These workflow inputs cannot be copied losslessly into a new browser request. The original run is preserved; no additional run was started.",
  regeneration_workflow_workspace_binding_required:"This workflow binds folders to its original chat. A new chat needs a matching explicit folder binding. No additional run was started.",
  regeneration_requires_model_answer:"Confirmed regeneration is not yet available for this execution type. Reuse its prompt in the appropriate task.",
  workflow_input_source_mismatch:"The original workflow inputs do not match the saved receipt. No replacement task was started.",
  workflow_input_source_requires_plan_v2:"The original workflow inputs require the confirmed execution review.",
  runCopyRestoreHint:"Restoration uses a confirmed plan and a new working copy. Original files and existing result versions remain unchanged.",
  runCopyRestoreUnavailable:"Download this file version or use “Branch from here” to continue with it in a new chat. Restoring individual files is not yet available here.",
  waiting_for_timer:"Scheduled continuation",
  workflowWaitingTimer:"Waiting for scheduled time",
  workflowNodeSkipped:"Skipped",
  saved_chat_provider_revision_unavailable:"This chat's saved model revision is no longer available. Explicitly choose a current model profile.",
  chat_configuration_not_saved:"The chat configuration is awaiting confirmation. Wait for the save or retry the original request.",
  library_source_unavailable:"The original library template is no longer available. The saved copy is retained.",
  chat_configuration_save_busy:"A chat configuration save is already in progress. Wait for its confirmation.",
  chat_configuration_unavailable:"The chat configuration could not be loaded. Check the connection and reload the server copy.",
  chat_configuration_receipt_mismatch:"The save confirmation does not match the original chat configuration. The request is retained for reconciliation.",
  chat_configuration_draft_conflict:"Another view changed this chat's configuration. Load the server copy or explicitly save your visible configuration draft.",
  chat_configuration_invalid:"The chat configuration contains invalid values. Check the draft before saving.",
  chat_configuration_credentials_forbidden:"Credentials cannot be stored in a local configuration draft. Use the provider or secret settings.",
  request_journal_invalid:"The local save reference is damaged. Explicitly discard it and reload the saved configuration.",
  request_journal_changed:"Another view has recorded a pending save request. No request was overwritten. Reconcile the saved state first.",
  request_journal_unavailable:"Local recovery storage is unavailable. The configuration could not be safely prepared.",
  default_provider_unavailable:"The saved default model is unavailable. Choose a valid profile in Teams & Tools.",
  default_agent_unavailable:"The saved default template is unavailable. Choose an agent in settings.",
  selected_skill_unavailable:"A selected skill was deleted or disabled. Update the agent template.",
  selected_memory_unavailable:"A selected knowledge entry is unavailable. Update the agent template.",
  selected_mcp_tool_unavailable:"A selected MCP tool changed or is unavailable. Update the agent template.",
  ...structuredOutputMessages.en,
  ...moneyMessages.en,
  ...connectionMessages.en,
  ...secretMessages.en,
  ...mcpMessages.en,
  ...workflowMessages.en,
  budget_configuration:"The token budget must cover at least one context window of the selected model. The reply limit must be smaller than that window.",context_limit:"The conversation and tool definitions exceed the current context limit. Choose a suitable model profile or start a new order with explicitly selected context.",reservationPerRequest:"Reservation per model request",
  runBudgets:"Limits for the next run",runBudgetStatus:"Token budget for this run",maxRunSteps:"Maximum agent steps",maxRunSeconds:"Maximum run time (seconds)",maxReplyTokens:"Maximum tokens per model reply",maxRunTokens:"Token budget for the whole run",budgetHint:"These limits are frozen when the run starts. Run time includes approval and input waits.",tokenReservationHint:"Each model request first reserves one full configured context window. Confirmed usage replaces the reservation; missing measurements remain held. Another request cannot start without enough remaining budget.",remainingTokenBudget:"Remaining token balance",unconfirmedTokenBudget:"Provisionally held within the charge",usageIncomplete:"Usage measurements are incomplete or pending. Reported token totals are not a complete account; unconfirmed reservations remain held.",providerBudgetOverrun:"The provider exceeded a limit. Further model and tool actions in this run are blocked.",

  manageFolderAccess:"Manage folder access",nativeFolderHint:"Choose an existing folder in the desktop and confirm its permissions. New grants default to read only. Select it for a run separately above.",grantFolderChanges:"Allow changes in the new folder too",chooseFolder:"Choose folder",refreshFolderAccess:"Refresh access",retryFolderChoice:"Reconcile folder selection",folderChoicePending:"The previous request is preserved. Retrying first checks whether its grant was already saved.",folderChoiceCanceled:"Folder selection canceled.",folderRegistered:"Folder granted. Explicitly select it above for the next run.",folderRevoked:"Folder access revoked. Files are retained.",folderAccessRevoked:"Access revoked",revokeFolderAccess:"Revoke access",confirmFolderRevocation:"Confirm revocation",folderRevokeHint:"After confirmation, no new file access may begin under this grant. Active native file operations must finish first. Uncertain earlier changes can still be reconciled; no files are deleted.",folderRevokeBusy:"The folder is in use or its grant is being updated. Wait for completion, then confirm revocation again.",folder_desktop_required:"Set up additional computer folders in the connected desktop. The browser can only select existing grants for a run.",folder_connection_failed:"Folder management is unavailable. Check the desktop connection and refresh access.",folder_picker_busy:"A native folder chooser is already open. Complete it first.",folder_registration_uncertain:"The folder change is unconfirmed. Refresh access or reconcile the same request again.",
  refreshUploads:"Refresh uploads",addFile:"Add file",uploadHint:"Choose a file up to 64 MiB. Upload stores a verified copy for this chat. Review and confirm its import into a new working copy afterward.",chooseLocalFile:"Choose a file",uploadFile:"Upload file",uploadBusy:"Verifying and uploading file …",uploadReadyHint:"File verified and stored. Choose “Import file” to review a new working copy.",uploadReady:"Upload confirmed",uploadReserved:"Upload pending",uploadContinue:"Continue upload",uploadedFiles:"Uploaded files",clearSelection:"Clear selection",uploadFailed:"The upload is unconfirmed. Refresh the list or retry with the same file.",uploadMismatch:"The selected file does not match the pending upload. Choose the original file or clear the selection.",upload_too_large:"This file exceeds the 64 MiB limit.",upload_integrity:"The file and upload receipt do not match. Working files were not changed automatically.",transfer_busy:"Two file transfers are already active. Wait for them to finish and try again.",
  import_file:"Import file",document_read:"Read document",document_write:"Create or change document",image_edit:"Edit image",generatedFile:"Generated new file",generatedBy:"Generated with",documentOperation:"Operation",documentInputs:"Source files",documentOutput:"New file",documentContent:"Content",documentNoOverwrite:"A new file is created. Existing files are never overwritten.",imageSteps:"Edit steps",documentPreviews:"Preview",previewUnavailable:"Preview unavailable",importHint:"A confirmed run imports the verified file into a new working copy. Originals and earlier results are preserved. No model is called.",importReadOnly:"Read-only mode does not permit file imports.",checkDestination:"Check destination",destinationExists:"An entry already exists at this destination.",destinationNew:"A new file will be created at this destination.",destinationDirectory:"The destination is a directory. Choose a file path.",confirmReplace:"Replace the inspected version in the new copy. The original file is preserved.",startImport:"Review import",importPending:"The start request is unconfirmed. Retrying requests the same run; destination and access mode remain frozen.",uploadedSource:"Uploaded source",
  fileSaved:"File saved and verified.",saveCanceled:"Save canceled.",save_busy:"A save dialog is already open. Finish it first.",save_forbidden:"This destination is not allowed. Choose a regular file outside private application data.",save_conflict:"The destination file or folder changed. Review it and choose the destination again.",save_invalid:"The file or its integrity metadata is invalid. Load the original content again.",save_failed:"The file could not be saved. Check disk space and access permissions.",save_uncertain:"File delivery has an unconfirmed outcome. Inspect the selected destination before saving again.",
  downloadCurrent:"Download current file",downloadSnapshot:"Download saved file",downloadChecking:"Loading and verifying file …",downloadPrepared:"Download prepared. Check your browser's download location.",downloadFailed:"The download could not be prepared safely. Check the connection and file access, then try again.",downloadChanged:"The file changed since the preview. Refresh the preview before downloading.",downloadBusy:"The files are being changed. Try the download again after the operation finishes.",downloadLimit:"Downloads are limited to 64 MiB per file.",workspacePreviewUnavailable:"No text preview is available for this file. Download access is checked separately.",
  restore_file:"Restore retained files",nativeFileAction:"Native file action · no model",restoreHint:"Restore retained content as an independent copy in the same granted folder. The destination must be absent and its parent directory must exist.",restoreApprovalHint:"This creates a separate run. Approve the exact action next. Your current draft is preserved; no model is required.",prepareRestore:"Prepare restoration",restorePrevious:"Restore previous version",restoreWritten:"Restore written version",
  stat_file:"Inspect file state",create_directory:"Create directory",rename_file:"Move file or directory",remove_file:"Remove file or directory",destinationPath:"Destination",removeFileHint:"The visible entry will be removed. The verified content is retained for recovery.",retainedFiles:"Retained entries",


  workingListTruncated:"Showing the first 500 entries.",

  workingFiles:"Working files",workingFilesHint:"This order keeps its current files for future runs. The selected access mode applies here too.",

  workingFilesEmpty:"No working files yet. The model can create files here when the selected access mode permits it.",

  workingFilesBusy:"Working files are currently in use. Retry after the operation finishes.",workingFilesUnavailable:"Working files could not be read.",folderUp:"Parent folder",refreshFiles:"Refresh files",blockedFile:"Not accessible",

  workspaceSnapshot:"Saved files for this run",workspaceSnapshotHint:"This snapshot belongs to the selected result. Branches receive an independent working copy.",workspaceInherited:"Working copy from the selected historical file snapshot",

  workspaceTextHint:"Preview supports UTF-8 text up to 1 MB. Binary files cannot currently be displayed.",



  filePath:"File in the granted folder",

  folderContext:"Folder context",noFolderAccess:"No additional folders selected",folderHint:"Select explicitly granted folders for the next run. The access mode also applies. Contents may be sent to the selected model.",

  noConfiguredFolders:"No additional folder is granted for this project yet.",

  folderWritable:"Read and approved changes",folderReadOnly:"Read only",folderGrantChanged:"A selected folder grant changed or was removed. Clear the selection and review the new permissions before starting.",clearFolders:"Clear selection",

  fileChanges:"File changes in this run",compareVersions:"Compare versions",previousVersion:"Previous version",proposedVersion:"Proposed version",newFile:"New file",versionUnavailable:"The retained version could not be read.",

  uncertainWrite:"Reconcile the file change",uncertainHint:"This change was started but its completion is unconfirmed. Reconcile the actual file before continuing.",

  reconcileHint:"The daemon checks content, identity and paths. Your decision is accepted only if the actual file state exactly matches the selected outcome. The run then stays interrupted until you resume it.",

  confirmNotApplied:"Confirm unchanged original",acceptCompleted:"Accept completed change",writeInProgress:"Completion of this change is being checked.",

  list_files:"List files",read_text_file:"Read text file",write_text_file:"Write text file",workspace_busy:"Another run owns this folder. Write access waits for it to be released.",file_conflict:"The file does not match the expected version. Review its current state before making changes.",uncertain_effect:"The last change has an unconfirmed outcome. Reconcile the file first.",executor_interrupted:"Execution was interrupted. Review its history before resuming.",

  activity:"Activity",inbox:"Inbox",allActive:"All open runs",activityHint:"Approvals, questions and ongoing work across all local projects.",

  inboxEmpty:"No pending approvals or questions. Interrupted runs also appear here.",activityEmpty:"There are no open runs.",

  searchHistory:"Search history",searchHistoryHint:"Search chat titles, messages and results.",searchScope:"Search scope",allProjects:"All projects",currentProject:"Current project",noSearchResults:"No matching content found.",

  previousPage:"Previous",nextPage:"Next",exportOrder:"Export order",exportHint:"Download the confirmed conversation, results, run history and approvals as JSON. The export includes an integrity hash and is not an executable migration package.",downloadExport:"Download JSON",

  branch:"Branch",branchHere:"Branch from here",branchHint:"A new order inherits the unchanged history through this reply and its results. An available file snapshot becomes an independent working copy; external folders stay unchanged. New conversations remain separate. Running actions and approvals are not transferred.",branchBoundary:"Selected final reply",createBranch:"Create branch",inheritedHistory:"Inherited history",inheritedHint:"This historical snapshot stays unchanged. New messages belong to this order.",openOrigin:"Open source order",historical:"Historical",

  app:"LocalAI Cowork",preview:"V2 · Development",workspace:"Workspace",projects:"Projects",orders:"Chats",

  newProject:"New project",newOrder:"New Chat",projectName:"Project name",orderName:"Chat title",create:"Create",close:"Close",cancel:"Cancel",save:"Save",

  settings:"Settings",connect:"Connect your workspace",connectText:"Connect your local workspace or your own V2 server. Projects and orders stay stored on the selected system.",

  token:"Connection token",tokenHelp:"Find the private token in daemon.token in the V2 data directory. It is kept only for this browser session.",connectAction:"Connect",

  disconnect:"Disconnect",local:"This computer",synced:"Connected",connecting:"Connecting",disconnected:"Disconnected",reconnect:"Reconnect",

  serverUnavailable:"The daemon is unavailable. Your confirmed data remains stored.",hello:"What would you like to work on?",

  intro:"A goal, an order, a result you can trace.",startProject:"Create a project",emptyProject:"A shared place for your orders and results. This project stays local.",

  startOrder:"Describe an order",emptyOrders:"No orders yet",emptyMessages:"Describe your goal and what a successful result looks like.",

  prompt:"What needs to be done?",promptPlaceholder:"For example: Write a clear summary …",send:"Start run",sending:"Starting …",

  saved:"Draft saved",unsaved:"Not saved yet",saving:"Saving draft …",

  draftConflict:"This order changed elsewhere. Your draft is preserved here. Review the stored version before replacing it.",

  useServerDraft:"Load stored draft",keepMyDraft:"Save my draft after review",noProvider:"Connect a model first",chooseProvider:"Model",

  providerSetup:"Connect a model",providerHint:"Enter an existing model. No models will be downloaded.",providerName:"Name",providerType:"Provider",

  endpoint:"Endpoint",model:"Model name",context:"Context limit (tokens)",

  credentialHint:"Providers requiring API keys are connected through providers.json and a native environment variable. Keys are not stored in the browser.",

  remoteHint:"Selecting a remote endpoint sends order content to that provider.",

  access:"Access mode",read_only:"Read only",confirm_mutations:"Confirm changes",confirm_all:"Confirm every action",autonomous:"Autonomous within granted access",

  scope:"Granted tools: read results and create text artifacts.",results:"Results",conversation:"Conversation",details:"Details",

  noResults:"Results appear here when a run creates them.",runs:"Runs",noRuns:"No runs yet",currentRun:"Run",you:"You",assistant:"Cowork",

  queued:"Queued",waiting_for_executor:"Waiting for executor",waiting_for_snapshot:"Preparing context",running:"Running",waiting_approval:"Approval required",

  waiting_input:"Waiting for input",cancel_requested:"Cancellation requested",interrupted:"Interrupted",completed:"Completed",failed:"Failed",

  canceled:"Canceled",expired:"Budget reached",stopRun:"Cancel run",resume:"Resume with review",retry:"Reload",approvalTitle:"Your approval is required",

  approvalDescription:"This decision applies only to this exact action and its displayed arguments.",approve:"Allow action",deny:"Deny action",

  question:"Question",answer:"Your answer",submitAnswer:"Send answer",create_artifact:"Create result",list_artifacts:"List results",read_artifact:"Read result",

  request_input:"Request input",diagnostics:"Technical details",download:"Download",artifactIntegrity:"SHA-256 and provenance",events:"Stored events",

  cost:"Cost",unknown:"Unknown",tokens:"Tokens (input / output)",steps:"Agent steps",search:"Search orders",openNavigation:"Open navigation",

  toggleResults:"Show results",toggleTheme:"Change appearance",light:"Light",dark:"Dark",language:"Language",loading:"Loading …",error:"The action could not be completed.",
  policy_denied:"Approval was denied or an applicable policy prevents this action.",

  revision_conflict:"The resource changed elsewhere. Reload and review the changes.",authentication_required:"The connection token is invalid.",

  forbidden:"This action is not granted.",invalid_request:"Please check your input.",provider_unavailable:"The selected model is unavailable. Check its name, endpoint and credentials.",
  provider_request_rejected:"The model server rejected the request (HTTP 400/422). Check the supported request parameters and chat format.",
  provider_authentication_failed:"The model server rejected the API key (HTTP 401).",
  provider_access_denied:"The model server denied access (HTTP 403).",
  provider_endpoint_not_found:"The API path was not found (HTTP 404). Check the endpoint, including /v1.",
  provider_rate_limited:"The model server is limiting requests (HTTP 429).",
  provider_connection_timeout:"The model server did not respond within the time limit.",
  provider_tls_failed:"The encrypted connection to the model server failed. Check its certificate.",
  provider_connection_failed:"The connection to the model server failed.",

  storage_unavailable:"Saving is unavailable. Unconfirmed changes remain visible as drafts.",stale_lease:"This worker no longer owns this run.",

  budget_exceeded:"The execution budget is exhausted. A new run requires a new configuration.",

  previewNote:"Local development preview · not qualified for enterprise",emptyProvider:"No model connected",localOnly:"Private · local",

  pending:"Open",selectRun:"Choose a run",begin:"Your workspace is ready",navigation:"Projects and orders",stream:"Current response",

  returnConversation:"Back to conversation",confirmDraft:"Stored draft",problemDetails:"Error details",selected:"Selected",runKeepsGoing:"Accepted runs keep running when you close this window.",

};

