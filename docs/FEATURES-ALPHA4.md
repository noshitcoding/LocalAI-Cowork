# Funktionsstand 2.0.0-alpha.4

Alpha.4 erweitert den persönlichen Arbeitsplatz und den Teammodus. Diese
Beschreibung nennt implementierte Quellen und Bedienwege. Der aktuelle
Durchlauf umfasst Generierung, Kompilierung und Paketbau mit Byte- und
Quellbindungen; neue Tests, Installationen und Laufzeitabnahmen gehören nicht
dazu. Nachweise älterer Alpha-Versionen bleiben ausschließlich historisch.

Die neue Lieferung liegt in
`dist/2.0.0-alpha.4-2026-10-10-7d032f6e`. Ihre tatsächlichen Dateien
und Prüfsummen stehen in `SHA256SUMS`, die Grenzen
des Nachweises in `DELIVERY-RECEIPT.json`. Die Pakete sind unsigniert.

## Persönlicher Arbeitsplatz und Teams

- Persönliche Projekte, Aufträge, Gesprächsverläufe, gespeicherte Entwürfe,
  Modellläufe, Streaming, Abbruch, konkrete Werkzeugfreigaben und Artefakte.
  Native Ordnerfreigaben und Dateioperationen gehören zum lokalen Arbeitsplatz.
- Teamkonten mit Registrierung, Anmeldung und widerrufbaren Sitzungen;
  Owner/Admin/Member/Viewer sowie getrennte Projekt-Lese-, Schreib- und
  Ausführungsrechte. Der private Daemon-Verbindungsschlüssel ersetzt diese
  Teamrechte nicht.
- Teamprojekte und Aufträge erstellen, bearbeiten, archivieren und wieder
  aktivieren. Archivierte Inhalte bleiben lesbar; neue Schreib- und
  Ausführungsaktionen verlangen ein aktives Projekt bzw. einen aktiven Auftrag.
- Je Team eine verschlüsselt gespeicherte LiteLLM-Verbindung mit eigenem
  API-Schlüssel. Limits und Verbrauch verwaltet LiteLLM. Das Team verwendet
  seine konfigurierte Verbindung; es gibt keinen stillen Providerwechsel.
- Einzelagent sowie sequenzielle, parallele und hierarchische Teamabläufe mit
  gespeicherten Schrittergebnissen. Persönliche versionierte Workflows und
  ausdrückliche Antworten auf Eingabeanforderungen bestehen daneben.
- Projektbezogene Skills, Wissen und Memory mit Bearbeitung, Suche und Löschung
  bzw. Archivierung. Neue Läufe erhalten den ausgewählten, versionsgebundenen
  Kontext; historische oder importierte Inhalte erteilen keine Werkzeugrechte.

## Zeitpläne einschließlich Team-/LiteLLM-Läufen

Persönliche und Team-Zeitpläne unterstützen einmalige, tägliche, wöchentliche
und Intervall-Termine mit IANA-Zeitzonen und Sommerzeitbehandlung. Für Teams
bietet **Zeitpläne** Anlegen, Bearbeiten, Pausieren, erneute Freigabe, Löschen
und eine Laufhistorie. Pro Zeitplan ist höchstens eine Ausführung aktiv.

Eine Teamfreigabe entsteht aus den aktuellen Ausführungsrechten des angemeldeten
Benutzers und bleibt als eigener dauerhafter Grant gespeichert. Der Scheduler
verwendet keinen gespeicherten Login-Bearer. Mitgliedschaft, Projekt, Auftrag,
Provider und Freigaberevision werden bei Aufnahme und Dispatch erneut geprüft.
Eine Änderung dieser Bindungen kann den Zeitplan pausieren und eine erneute
ausdrückliche Freigabe erfordern. Der Daemon mit Worker muss für die Ausführung
laufen; ein geschlossener bzw. ausgeschalteter Rechner führt keine Termine aus.

Vorkommnis und Run-ID sind dauerhaft verknüpft. Automatische Teamläufe erhalten
keine Terminal-, Browser-, Office-, GitHub- oder MCP-Werkzeugfreigabe. Ein
unklarer Aufnahme- oder Ausführungsausgang pausiert den Zeitplan und wird nicht
automatisch wiederholt. Pausieren verhindert weitere Aufnahmen und ist kein
Abbruch eines bereits aufgenommenen Laufs. Aktive oder unklare Vorkommnisse
sperren Bearbeitung und Löschung bis zur geklärten Ausführung.

## Werkzeuge, Team-MCP und privates Team-GitHub

Terminal, Browser, Office/PDF und GitHub sind ausdrücklich auswählbare
Werkzeuge. Jede konkrete Modellaktion benötigt ihre eigene Freigabe und ein
dauerhaftes Ausführungsjournal. Die manuelle Werkzeugoberfläche bietet Vorschau,
bestätigte Ausführung und Arbeitsdateien. Browseraktionen verwenden ein eigenes
Auftragsprofil; Officeadapter unterstützen Text, DOCX, XLSX, PDF und PPTX sowie
eine eingerichtete LibreOffice-Konvertierung.

Unter **Teams & Werkzeuge → MCP & GitHub** können Teamadministratoren eigene
MCP-Server und GitHub-Zugangsdaten konfigurieren. Gespeicherte Schlüssel werden
nicht an die Formulare zurückgegeben.

- Team-MCP verwendet Streamable HTTP. Der Betreiber muss erlaubte Origins über
  `COWORK_TEAM_MCP_ALLOWED_ORIGINS` ausdrücklich vorgeben; die leere Vorgabe
  erlaubt keine Verbindung. Nach dem Speichern wird der Werkzeugkatalog mit
  **Katalog abrufen** geladen. Beim Laufstart werden die gewünschten Werkzeuge
  ausgewählt. Serverrevision, Katalog und konkrete Aktion sind gebunden;
  Änderungen können die Ausführung sperren. MCP benötigt keinen Teamcontainer.
- Privates Team-GitHub verwendet den verschlüsselten Token dieses Teams, den
  eingestellten Host und die freigegebene Aktion. Es benötigt einen eingerichteten
  Teamcontainer mit ausdrücklich zugelassenem Netzwerk. Der Token wird nur für
  diesen GitHub-Gast bereitgestellt. Betreiber-Token und vorhandene
  GitHub-Benutzerprofile werden nicht übernommen. Der Adapter erlaubt begrenzte
  Remote-API-Verwaltung; authentifiziertes Clone/Checkout/Sync und lokale
  Repository-Hooks gehören nicht zu seinem Umfang.

Persönliche native Werkzeuge verwenden die Betriebssystemrechte des Daemons.
Teamwerkzeuge benötigen ein vom Betreiber ausgewähltes separates Image und
Arbeitsverzeichnis. Nur der vertrauenswürdige Worker erhält Docker-Steuerung;
Gäste erhalten den jeweiligen Auftragsworkspace, keine Datenbank, keinen
Docker-Socket und keine Daemon-Schlüssel. Gastnetzwerk ist standardmäßig `none`.
Diese Implementierung ist keine praktisch abgenommene Sandbox.

## Dateien, Archive und V1-Übernahme

Teamdateien besitzen gespeicherte Versionen, Größe und SHA-256 sowie Upload,
Download, Löschung und ausdrücklichen Abgleich unklarer Übertragungen. Die Ablage
verwendet den privaten lokalen Speicher oder ein vom Betreiber konfiguriertes
S3-kompatibles Backend. Ein Teamupload ist auf 32 MiB begrenzt. Alle Teamzugriffe
prüfen die aktuelle Projektberechtigung.

Der versionierte V2-ZIP-Import/Export enthält Projekte, Auftragsentwürfe, Skills,
Wissen, Zeitpläne sowie die vorgesehenen Gesprächs- und Artefaktinhalte. Die UI
erlaubt neue Kopien oder die Ablehnung von ID-Konflikten. Importierte Zeitpläne
bleiben pausiert. Dieses Inhaltsarchiv ist kein vollständiges Datenbank-,
Schlüssel- oder S3-Backup.

Alpha.4 ergänzt die ausdrückliche V1-Inhaltsübernahme:

- Native Auswahl einer bekannten Desktop- oder Daemon-JSON-/SQLite-Quelle;
  Vorschau und gesonderte Bestätigung des gespeicherten Importplans.
- Optional ein zweiter nativer Picker für den Original-Anhangordner. Bekannte
  Artefaktpfade müssen innerhalb dieses gewählten Roots liegen. Kopierte Bytes
  werden mit Größe, SHA-256 und Aufnahmeidentität gebunden. Fehlende,
  unzulässige oder nicht unterstützte Anhänge erscheinen mit einem Grund in
  der Vorschau.
- Persönliche Übernahme in neue Projekte/Aufträge oder ausdrückliche Zuordnung
  zu bestehenden Teamprojekten. Jedes Teamziel verlangt zusätzlich die aktuelle
  Teamsitzung, Schreibrechte und passende Projektrevisionen.
- Historische Projekt-, Daemon-, Tenant-, Actor- und Persona-Kennungen bleiben
  getrennte, inerte Herkunftsdaten. Neue V2-IDs und aktuelle Rechte gelten für
  die übernommenen Inhalte. Nachrichten, Artefakttexte und Originalanhänge
  sind anschließend im importierten Verlauf lesbar bzw. herunterladbar.

Die V1-Datenbank wird nicht am Original mit SQLite geöffnet. Windows verwendet
native Schreib-/Löschsperren und Quellenbindungen; Linux verlangt eine
ausdrücklich kalte Quelle ohne WAL/SHM/Journal und prüft eine private Rohkopie
mit wiederholten Hashes. Dies sind unterschiedliche Aufnahmegrenzen.
Der Inhaltsimport ist begrenzt auf 100 Projekte, 2.000 Aufträge, 10.000
Nachrichten, 1.000 Artefakte und 16 MiB Text. Originalanhänge sind zusätzlich
auf 1.000 Dateien, 16 MiB pro Datei und insgesamt 32 MiB sowie unterstützte
Dokument-, Bild- und Textmedien begrenzt. Die gesamte private JSON-Projektion
einschließlich Base64-Anhängen ist zusätzlich auf 64 MiB begrenzt. Eine
SQLite-Quelldatei bis 512 MiB erlaubt entsprechend keinen Import von 512 MiB
Nutzinhalten.
Bekannte historische Dateigrößen müssen zum gewählten Anhang passen. Fehlt ein
historischer Dateihash, belegt der neue Aufnahmehash die kopierten aktuellen
Bytes, aber keine Gleichheit mit einer früheren gleich großen Dateiversion.

Alpha.4 ergänzt einen funktionalen Migrationskatalog mit Benutzern,
Organisationen, Mitgliedschaften, Projektberechtigungen und versionierten
Skills, Personas, Wissen/Memory, Provider-, Workflow- und Zeitplandefinitionen.
Ein ausdrücklicher Zuordnungsplan verbindet die historischen Kennungen mit
aktuellen V2-Projekten und vorhandenen oder neu angelegten V2-Benutzern.
Account-, Mitgliedschafts- und Inhaltsrechte werden unter heutigen
Administratorrechten mit Revisionsprüfung übernommen. Neue Konten erhalten
neue Passwörter; Provider erhalten einen neu eingegebenen, aktuell verschlüsselten
Schlüssel. Passwörter und Schlüssel gehören ausschließlich zur Bestätigung,
nicht zum gespeicherten Plan oder Quellkatalog.

Skills, Persona-Anweisungen und Wissen werden als verwendbare V2-Dokumente
bereitgestellt. Importierte Workflow- und Zeitplandefinitionen bleiben pausiert
und können mit aktuellen Auftrag-, Provider- und Ausführungsrechten ausdrücklich
neu aktiviert werden. Historische Versionen, Kennungen und Betriebszustände
bleiben nachvollziehbar. Alte Bearer, Verschlüsselungsschlüssel, offene Effekte,
aktive Workerleases und Ausführungsfreigaben werden nicht reaktiviert.

## Begrenzte Offline-Ausführung

**Offline-Ausführung** ist in der nativen Desktop-App auch ohne Teamsitzung
zugänglich. Bei bestehender Verbindung erteilt der Benutzer eine signierte,
ablaufende Gerätefreigabe für genau ein Teamprojekt und einen Auftrag. Sie bindet
Geräteschlüssel, Prompt, eingefrorene Quellen, die ursprüngliche
Provider-/Berechtigungsrevision und den ausdrücklich gewählten lokalen
Modellendpunkt samt Modell. Die Freigabe begrenzt Laufanzahl und Dauer.
Token- und Geldlimits bleiben bei LiteLLM.

Der eigene Gerätemodellschlüssel wird nativ verschlüsselt gespeichert. Der
Teamprovider-Schlüssel wird nicht exportiert. Ein ausdrücklich schlüsselloser
lokaler Ollama-Endpunkt ist ebenfalls möglich. Die Ausführung benötigt keinen
Cowork-Teamserverkontakt, jedoch den erreichbaren freigegebenen Modelldienst.
`list_sources`, `read_source` und `create_artifact` sind begrenzte Offlinewerkzeuge;
konkrete Aktionen benötigen ihre eigene Freigabe. Textartefakte, Modellantworten
und Ausführungsjournal entstehen tatsächlich auf dem Gerät.

Lokale Freigaben, Laufzusammenfassungen und einzelne Verläufe sind dauerhaft
abrufbar. Unklare Ausgänge werden nicht automatisch wiederholt. Beim
Wiederverbinden ist ein ausdrücklicher Abgleich möglich; dieser prüft aktuelle
Mitgliedschaft, Inhalts-/Ausführungsrechte und gebundene Revisionen und importiert
einen signierten Verlauf höchstens einmal. Ein ausgeschaltetes Gerät empfängt
keinen Widerruf: Offline bleibt die signierte Ablaufzeit maßgeblich, bis eine
Onlineprüfung einen Widerruf oder eine Änderung bestätigt.

Die bereits vorhandenen Offline-Inhaltsentwürfe und der Projekt-Sync mit
monotonem Cursor bleiben zusätzlich verfügbar.

## Android, Passkeys, Push und Geräteausführung

Der `/api/v1`-Adapter verbindet Android-Schema 2 mit aktuellen V2-Teamkonten:
Anmeldung/PKCE, Sitzungswechsel/Widerruf, Projekte, Threads, Läufe, Nachrichten,
SSE-Cursor und Anhänge. Alpha.4 ergänzt echte WebAuthn-Passkeyregistrierung,
Anmeldung und Löschung sowie den nativen PKCE-Passkeyweg. Der Betreiber muss
HTTPS-Origin und den genauen RP-Host gemeinsam konfigurieren. Challenges und
Zähler bleiben serverseitig; eine Benutzernennung ersetzt keinen Passkeybeweis.

Push-Abonnements sind verschlüsselt und widerrufbar. Der optionale FCM-v1-Worker
verwendet die private Serviceaccount-Datei des Betreibers und echte
OAuth-/Googleanfragen. Ohne diese Datei bleibt die Zustellung sichtbar ausstehend.
Eine von Google angenommene Nachricht ist keine Bestätigung ihrer Anzeige am
Telefon.

Die Geräteoberfläche bietet ausdrückliche Pairung, eigene ablaufende
Gerätecredentials, Heartbeats und Widerruf. Geräteaufträge verwenden eigene
dauerhafte Leases, Ereignisse und Abschluss-/Fehlermeldungen; ein serverseitiger
Teamrun wird dadurch nicht nochmals ausgeführt. Der Geräteagent benötigt seine
eigene ausdrücklich konfigurierte Modellverbindung. Desktopübernahme verwendet
Runbindung, erneute Autorisierung und einmalige Streamtickets für die binäre
RFB-Verbindung zum authentifizierten Agenten. Ein unbestätigter physischer Stopp
wird als unterbrochen dargestellt.

Eine kompatible bereits vorhandene Android-App und ein ausführbarer Geräteagent
müssen vom Benutzer bereitgestellt werden. Diese Lieferung enthält keinen neuen
Android-APK und keine automatische Geräte- oder VM-Provisionierung.

## Installation und verbleibende Grenzen

Die [Alpha-4-Installationsanleitung](INSTALLATION-ALPHA4.md) beschreibt lokalen
Desktop, Teamkonten und den eigenen Linux-Server. Die Lieferung umfasst auch
Compose-/Kubernetes-Vorlagen, S3-Konfiguration und Betreiberbefehle für
Backup/Restore/Update/Rollback. Diese werden erst durch den Betreiber ausgeführt.
Mehrknotenbetrieb benötigt tatsächlich geteilte Arbeitsdateien, passende
Schlüsselhoheit und qualifizierte Dateisystem-/Locksemantik; eine HA-Vorlage ist
kein HA-Nachweis.

Signierung, native Installation, produktive Isolation, S3-Betrieb, Last,
Failover, vollständiges M0–M10 und die gemeinsame Abnahme mit einem bestehenden
V1-/Open-Cowork-Server bleiben durch diesen Build-Durchlauf unbestätigt. Es gibt
keine macOS-Lieferung und keinen bereitgestellten öffentlichen SaaS-Dienst.
Unbekannte externe Wirkungen benötigen weiterhin ausdrückliche Klärung.

Die Implementierungsbezüge liegen in den mitgelieferten Quellen:
`team_schedules.rs`, `team_service.rs`, `team_integrations.rs`,
`team_blobs.rs`, `productivity*.rs`, `import_basis.rs`, `import_team.rs`,
`import_attachments.rs`, `import_catalog.rs`, `offline_execution.rs`,
`android_compat_routes.rs`, `android_device_services.rs`, den entsprechenden
Daemonrouten und den UI-Komponenten unter `packages/ui/src/`.
