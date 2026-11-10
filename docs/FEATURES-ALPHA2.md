# Funktionsstand 2.0.0-alpha.2

Dieser Stand setzt den Auftrag zur schnellen Funktionsimplementierung um. Neue
Tests, Lastläufe und Installationsabnahmen sind auf ausdrücklichen Wunsch nicht
Teil dieses Durchlaufs. Compiler-, TypeScript- und Paketbaufehler werden behoben.
Die früheren alpha.1- und Testnachweise gehören ausschließlich zu ihren alten
Quellen und Artefakten.

## Bedienbare Funktionen

- Teams mit eigenen Benutzerkonten, Registrierung/Anmeldung, Mitgliedern,
  Owner/Admin/Member/Viewer und getrennten Projekt-Inhaltsrechten.
- Eine verschlüsselt gespeicherte LiteLLM-Verbindung mit eigenem API-Schlüssel
  je Team. Token- und Geldlimits verwaltet das vorhandene LiteLLM; neue interne
  Teambudgets werden nicht benötigt.
- Teamprojekte und Aufträge erstellen, umbenennen, archivieren und wieder
  aktivieren; gespeicherte Antworten und Ablaufschritte. Einzelagent,
  sequenzieller und paralleler Graph sowie hierarchischer Worker-/Managerablauf.
- Persönliche versionierte Workflows mit festem Manager-Schritt, der die
  bestätigten Ergebnisse aller Worker erhält. Dynamisch vom Modell erzeugte
  Childruns sind davon getrennt.
- Terminal, Browser, Office/PDF und GitHub als auswählbare Modellwerkzeuge mit
  konkreter Aktionsfreigabe und dauerhaftem Ausführungsjournal. Zusätzlich
  manuelle Werkzeugbedienung und Übertragung ihrer Arbeitsdateien.
- Projekt-Skills sowie durchsuchbares Wissen und Memory. Aktive Skills und
  passende Referenzen werden beim Start für den neuen Lauf eingefroren.
- Persönliche einmalige, tägliche, wöchentliche und Intervall-Zeitpläne mit IANA-Zeitzonen,
  Sommerzeitbehandlung, Pausierung und jeweils einer aktiven Ausführung.
- Versionierte Teamdateien in privater lokaler Ablage oder S3, mit Download,
  SHA-256, Löschjournal und ausdrücklichem Abgleich unklarer Übertragungen.
- V2-ZIP-Import/Export einschließlich Gesprächs- und Artefaktinhalten. Importierte
  Verläufe sind lesbar; importierte Zeitpläne bleiben zunächst pausiert.
- Native Vorschau und ausdrückliche Übernahme alter V1-JSON-/SQLite-Inhalte,
  einschließlich lesbarer historischer Nachrichten und Artefaktvorschauen.
- Geräteverwaltung, Widerruf, monotoner Projekt-Sync und begrenzte lokale
  Offline-Entwürfe. Übertragung und Commit erfolgen ausdrücklich; unklare
  Modell- oder Werkzeugaktionen werden nicht automatisch erneut ausgeführt.
- Kernkompatibilität mit dem vorhandenen Android-Schema2-Client: PKCE-Anmeldung,
  Projekte, Threads, Läufe, Nachrichten, SSE mit Wiederverbindung, Anhänge und
  widerrufbare Sitzungen.
- Selfhosting-Dateien für getrennte API/Worker, PostgreSQL, TLS, optionales S3,
  Kubernetes einschließlich einer HA-Vorlage und betreiberseitigem
  Backup/Restore/Update/Rollback.

## Installation und Einrichtung

Windows verwendet den NSIS-Installer, Linux das DEB oder AppImage. Die App
verbindet sich wahlweise mit ihrem lokalen Daemon oder einem eigenen V2-Server.
Für normale Teamkonten im Verbindungsdialog **Mit Teamkonto anmelden** wählen.
Die Ersteinrichtung verwendet den privaten Einrichtungsschlüssel des Betreibers
aus `data/identity-bootstrap/master.key`; danach reicht die eigene Anmeldung.

Unter **Teams & Werkzeuge → Team & LiteLLM** Basis-URL, Modell und den vorhandenen
Team-API-Schlüssel eintragen. HTTPS ist der Standard. Für ausdrücklich gewählte
interne HTTP-LiteLLM-Origins kann der Betreiber
`COWORK_TEAM_LITELLM_HTTP_ORIGINS=http://litellm:4000` setzen; Pfade und Wildcards
sind ausgeschlossen. Die Team-URL darf anschließend beispielsweise
`http://litellm:4000/v1` sein.

Persönliche Terminal- und GitHub-Werkzeuge verwenden installierte und freigegebene Programme.
Interaktive Browseraktionen benötigen Python/Playwright; PDF, Tabellen und
PowerPoint benötigen die entsprechenden Pythonmodule. Das Serverimage enthält
diese Abhängigkeiten. Für den Desktop richtet
`python tools/toolbox/install.py --data-dir <eigener-V2-Datenordner>` eine
getrennte Umgebung ein. Git, `gh` und LibreOffice für Konvertierung werden
bei Bedarf separat installiert. GitHub-Zugang erfolgt über eine ausdrücklich
gewählte private Tokendatei in `toolbox.json`, nicht über fremde Benutzerprofile.

Teamwerkzeuge benötigen einen vom Betreiber eingerichteten getrennten Container.
Ohne `COWORK_TEAM_TOOL_IMAGE` werden sie nicht angeboten. Das Werkzeugimage
enthält Python, Node, Git, Browser und Officeadapter; nur das Arbeitsverzeichnis
des jeweiligen Auftrags wird eingebunden. API und Worker verwenden denselben
Workspace, Docker-Steuerung erhält ausschließlich der vertrauenswürdige Worker.
Das Netzwerk ist standardmäßig abgeschaltet. Private GitHub-Zugänge pro Team
werden derzeit nicht angeboten.

Für Serverinstallation und Lebenszyklus siehe [Selfhosting](../deploy/v2/README.md).
Private Master-Schlüssel gehören zusammen mit der Datenbank ins Backup; ohne
sie lassen sich verschlüsselte Teamschlüssel nach einer Wiederherstellung nicht
entschlüsseln.

Lokale persönliche Arbeit kann mit einem lokal installierten Modell ohne
Serververbindung erfolgen. Im Teammodus werden Offline-Inhalte als lokale
Entwürfe gehalten und später ausdrücklich übertragen; entfernte Modell- und
Werkzeugaktionen werden nicht offline simuliert.

## Noch fehlende Funktionen im vollständigen Ausbau

- Zeitpläne für Team-/LiteLLM-Läufe; der vorhandene Scheduler arbeitet mit
  persönlichen Läufen.
- MCP-Werkzeuge innerhalb von Teamläufen; die persönliche MCP-Anbindung
  besteht bereits.
- Private GitHub-Zugangsdaten pro Team; persönliche GitHub-Zugänge sind möglich.
- Vollständige V1-Datenparität mit Daemon-/Tenant-/Actor-Zuordnungen und originalen
  Binäranhängen. Der aktuelle Import übernimmt ausgewählte persönliche Inhalte,
  Nachrichten und eingebettete Artefakttexte.

Diese Punkte sind Funktionslücken und werden nicht durch ausgelassene Tests
als erledigt betrachtet.

## Grenzen dieses schnellen Standes

Persönliche native Werkzeuge laufen mit den Betriebssystemrechten des
Daemon-Benutzers. Teamwerkzeuge laufen im getrennten Container mit begrenzter
CPU, RAM und Prozesszahl; Diskquoten, Netzrichtlinien und ein optionales
gVisor-Runtime müssen vom Betreiber bereitgestellt werden. Die Isolation wurde
in diesem Durchlauf nicht praktisch qualifiziert. Bestehende signierte Runner-/Umgebungs-
Schnittstellen bleiben gesonderte Betreiberfunktionen. Hohe Verfügbarkeit,
produktive Isolation, Signierung, native Installation und die vollständige
M0–M10-/gemeinsame Serverabnahme sind durch reine Builds nicht bestätigt.
Unbekannte externe Wirkungen bleiben gesperrt, bis sie ausdrücklich abgeglichen
werden können. Eine neue Android-Oberfläche, macOS und ein öffentlicher
SaaS-Betrieb gehören nicht zu dieser Lieferung.
