# Funktionsstand 2.0.0-alpha.3

Alpha.3 erweitert den persönlichen Arbeitsplatz und den Teammodus. Diese
Beschreibung nennt implementierte Quellen und Bedienwege. Der aktuelle
Durchlauf umfasst Generierung, Kompilierung und Paketbau mit Byte- und
Quellbindungen; neue Tests, Installationen und Laufzeitabnahmen gehören nicht
dazu. Nachweise älterer Alpha-Versionen bleiben ausschließlich historisch.

Die neue Lieferung ist für
`dist/2.0.0-alpha.3-2026-10-10-916cb104` vorgesehen. Ihre tatsächlichen Dateien
und Prüfsummen stehen nach der Zusammenstellung in `SHA256SUMS`, die Grenzen
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

Alpha.3 ergänzt die ausdrückliche V1-Inhaltsübernahme:

- Native Auswahl einer bekannten Desktop- oder Daemon-JSON-/SQLite-Quelle;
  Vorschau und gesonderte Bestätigung des gespeicherten Importplans.
- Optional ein zweiter nativer Picker für den Original-Anhangordner. Bekannte
  Artefaktpfade müssen innerhalb dieses gewählten Roots liegen. Kopierte Bytes
  werden mit Größe, SHA-256 und Aufnahmeidentität gebunden. Fehlende,
  unzulässige oder nicht unterstützte Anhänge erscheinen mit einem Grund in
  der Vorschau.
- Persönliche Übernahme in neue Projekte/Aufträge oder ausdrückliche Übernahme
  in ein bestehendes Teamprojekt. Das Teamziel verlangt zusätzlich die aktuelle
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

Alte Principals, ACLs, Schlüssel, Freigaben, ausführbare RunSpecs, laufende
Aufträge, Scheduler und Replayrechte werden nicht übernommen. Der native
Quellpicker steht beim lokalen Desktop-Daemon zur Verfügung; ein Browser oder
eine Remote-Verbindung kann damit keine beliebigen Hostdateien lesen. Die
Übernahme ist keine vollständige V1-Daten-, Betriebs- oder Buildparität.

## Geräte, Offline-Entwürfe und Android-Kernadapter

Geräte können registriert und widerrufen werden. Projekt-Sync verwendet einen
monotonen Cursor. Die lokale Offline-Warteschlange hält höchstens 100
Inhaltsentwürfe mit insgesamt 4 MiB und verlangt ausdrückliche Übertragung und
Commit. Offline wird keine Modell- oder Werkzeugausführung simuliert; unklare
externe Wirkungen werden nicht automatisch erneut ausgelöst.

Der `/api/v1`-Adapter bildet Android-Schema-2-Kernfunktionen auf aktuelle
V2-Teamkonten und Projektberechtigungen ab: Anmeldung einschließlich PKCE,
Sitzungswechsel/Widerruf, Projekte, Threads, Läufe, Nachrichten, beobachtete
Run-Events per SSE mit Cursor und Anhänge. Unbekannte Routen sind ausdrücklich
nicht unterstützt. Ein Android-APK, FCM, Passkeys, Fleet-/Desktopübernahme und
vollständige mobile V1-Parität sind nicht enthalten.

## Installation und verbleibende Grenzen

Die [Alpha-3-Installationsanleitung](INSTALLATION-ALPHA3.md) beschreibt lokalen
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
`import_attachments.rs`, `android_compat_routes.rs`, den entsprechenden
Daemonrouten und den UI-Komponenten unter `packages/ui/src/`.
