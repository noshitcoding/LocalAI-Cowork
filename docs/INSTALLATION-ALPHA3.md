# Alpha.3 installieren und einen eigenen Server einrichten

Version: `2.0.0-alpha.3`. Vorgesehener Lieferordner:
`dist/2.0.0-alpha.3-2026-10-10-916cb104`.
Das Installations-ZIP liegt daneben. Der Ordner enthält Desktoppakete,
Linux-Server-Tar, Webfrontend, Deploymentvorlagen, Werkzeughelfer, Quellen und
öffentliche Buildquittungen. Die tatsächlich zusammengestellten Dateinamen und
Hashes stehen in `SHA256SUMS`.

Dies ist eine unsignierte Alpha. Der aktuelle Durchlauf baut die Artefakte;
er installiert oder startet sie nicht und bestätigt keine neue Laufzeit-,
Sandbox-, HA- oder vollständige V1-Abnahme. Historische Alpha.1-/Alpha.2-Berichte
qualifizieren diese neuen Bytes nicht. Der [Funktionsstand](FEATURES-ALPHA3.md)
beschreibt implementierte Bedienwege und Grenzen.

## Lieferung prüfen

ZIP in einen eigenen Ordner entpacken. Den SHA-256 des Installations-ZIP mit
seiner separat gelieferten `.sha256`-Datei vergleichen; danach die gelieferten
Dateien gegen `SHA256SUMS` prüfen. Unter Linux geht dies im Lieferordner mit:

```bash
sha256sum --check SHA256SUMS
```

Unter Windows liefert `Get-FileHash -Algorithm SHA256 -LiteralPath '<Datei>'`
den Hash einer ausgewählten Datei. `DELIVERY-RECEIPT.json` nennt den
Nachweisumfang `build_only`. Prüfsummen und Quellbindungen sind keine
Herausgebersignatur. `SOURCES.zip` enthält die ausgewählten Produktquellen und
synthetische Fixtures, keine privaten Schlüssel oder Benutzerdaten.

## Desktop ohne externen Server

Windows x64 verwendet den gelieferten NSIS-Installer; Microsoft Edge WebView2
muss verfügbar sein. Die unsignierte Anwendung kann eine Windows-Warnung
auslösen. Linux x64 verwendet DEB oder AppImage; die Desktopbytes benötigen
glibc mindestens 2.39, GTK3 und WebKitGTK 4.1, entsprechend Ubuntu 24.04.
Ein AppImage hebt die glibc-Mindestversion nicht auf. Für das DEB kann der
Benutzer im Paketordner ausführen:

```bash
sudo apt install './LocalAI Cowork V2 Preview_2.0.0-alpha.3_amd64.deb'
```

Für das AppImage die passende gelieferte Datei ausführbar machen und starten:

```bash
chmod +x './LocalAI Cowork V2 Preview_2.0.0-alpha.3_amd64.AppImage'
'./LocalAI Cowork V2 Preview_2.0.0-alpha.3_amd64.AppImage'
```

Die Desktop-App enthält ihren passenden Daemon und öffnet den lokalen
Arbeitsplatz. Im Verbindungsdialog heißt die Auswahl **Lokalen Arbeitsplatz
öffnen**. Standardport ist `127.0.0.1:43821`. Ein anderer inkompatibler Dienst
auf diesem Port verhindert die lokale Verbindung. Die App verwendet ihren
eigenen V2-Datenordner; `COWORK_DATA_DIR` kann vor dem Start einen ausdrücklich
gewählten absoluten privaten Ordner festlegen. Bestehende Dateien müssen die
Besitz- und Rechteprüfungen erfüllen. Unsichere Dateien werden nicht automatisch
repariert oder Schlüssel still ersetzt.

Beim Schließen des Desktopfensters kann der Daemon weiterlaufen. Ein Update
ersetzt deshalb keinen noch laufenden fremden oder älteren Daemon automatisch.
Vor einem Versionswechsel den eigenen Daemon geordnet beenden und die eigenen
Daten bei gestoppten Schreibern sichern. V1-Datenbanken niemals als
`COWORK_DATABASE_URL` oder als V2-Datenbank verwenden.

Für persönliche Modellarbeit einen bereits vorhandenen Ollama- oder
OpenAI-kompatiblen Provider einrichten und ein dort verfügbares Modell wählen.
OpenAI-kompatible Basis-URLs enthalten üblicherweise `/v1`; Remote-Verbindungen
benötigen HTTPS. Modelle werden nicht automatisch heruntergeladen. Lokale
Modellarbeit benötigt keinen externen Cowork-Server, aber den erreichbaren
gewählten Modelldienst.

## Teamkonto und LiteLLM

Im Verbindungsdialog **Mit Teamkonto anmelden** wählen. Zur ersten Einrichtung
**Ersteinrichtung** öffnen und den privaten Einrichtungsschlüssel des eigenen
Betreibers verwenden. Er liegt standardmäßig unter
`<V2-Datenordner>/identity-bootstrap/master.key`. Nur der Betreiber verwendet
diesen Schlüssel; normale Mitglieder erstellen ihr Konto und melden sich mit
Benutzername und Passwort an. Registrierung allein erteilt keine
Teammitgliedschaft. Der Administrator fügt die Benutzer-ID unter
**Mitglieder & Rechte** hinzu und vergibt die benötigten Projektinhaltsrechte.

Unter **Teams & Werkzeuge → Team & LiteLLM** ein Team und Projekt anlegen, dann
Basis-URL, Modell und den vorhandenen API-Schlüssel dieses Teams eintragen.
Die App speichert den Schlüssel verschlüsselt; LiteLLM verwaltet Limits und
Verbrauch. Standard ist HTTPS. Für eine bewusst gewählte interne HTTP-Verbindung
muss der Betreiber beim Start von API und Worker einen exakten Origin setzen,
beispielsweise `COWORK_TEAM_LITELLM_HTTP_ORIGINS=http://litellm:4000`.
Die Team-URL kann danach `http://litellm:4000/v1` lauten. Pfade, Wildcards oder
ein beliebiger privater Host ersetzen diese Betreiberfreigabe nicht.

Für Team-Zeitpläne **Zeitpläne** öffnen, Auftrag, Prompt, IANA-Zeitzone und
Wiederholung wählen und die wiederkehrenden Modellläufe ausdrücklich erlauben.
**Zunächst pausiert speichern** verzögert die Aktivierung. Nach Änderungen an
Projekt, Auftrag, Provider oder Rechten kann Bearbeiten und erneute Freigabe
nötig sein. Der Worker muss laufen. Die automatische Vorlage enthält keine
Werkzeugfreigaben. Unklare Vorkommnisse werden nicht automatisch wiederholt.

## Werkzeuge, MCP und GitHub einrichten

Das Desktoppaket enthält keinen vollständigen Python-/Office-/Browsertoolstack.
Für persönliche Werkzeuge und die native V1-Inhaltsaufnahme kann der Benutzer
aus den entpackten Quellen mit vorhandenem Python eine getrennte Umgebung
einrichten. Der Helfer benötigt Python 3.12 oder neuer und lädt seine
Pythonmodule sowie Chromium ausdrücklich herunter:

```bash
python tools/toolbox/install.py --data-dir '<absoluter-eigener-V2-Datenordner>'
```

Unter Linux heißt der Pythonbefehl gewöhnlich `python3`. Daemon anschließend
neu starten, damit er die private Werkzeugkonfiguration einliest. Git, `gh`
und LibreOffice bei Bedarf separat installieren und ausdrücklich auswählen.
Ein persönlicher GitHub-Token wird als private Tokendatei in `toolbox.json`
konfiguriert; vorhandene Benutzerprofile werden nicht durchsucht. Persönliche
Werkzeuge besitzen die Betriebssystemrechte dieses Daemon-Benutzers.

Für Team-MCP den Betreiber-Origin vorgeben, etwa
`COWORK_TEAM_MCP_ALLOWED_ORIGINS=https://mcp.example.com`, anschließend unter
**MCP & GitHub** den vollständigen Streamable-HTTP-Endpunkt und bei Bedarf den
Team-Bearer eintragen. **Katalog abrufen** ausführen und beim Start eines
Auftrags nur die gewünschten Werkzeuge wählen. Eine konkrete Werkzeugaktion
muss danach zusätzlich freigegeben werden. MCP benötigt kein Docker-Gastimage.

Team-Terminal, Browser, Office und GitHub benötigen einen separat eingerichteten
Teamcontainer. Der Betreiber wählt ein unveränderliches Werkzeugimage, den
privaten Docker-Hostworkspace und gegebenenfalls ein eingeschränktes
Gastnetzwerk. Standardnetzwerk ist `none`; GitHub und Browsernetzwerkaktionen
benötigen eine ausdrückliche Netzwerkoption. Unter **MCP & GitHub** den eigenen
Team-GitHub-Host und Token speichern und aktivieren. Ein angebotener
GitHub-Werkzeugeintrag verlangt eingerichtete Teamcredentials und passende
Containerkonfiguration. Der Teamtoken wird ausschließlich für die bestätigte
GitHub-Gastaktion weitergegeben, niemals ein Betreiber-Token.

## Eigener V2-Server ohne Docker

Das gelieferte `server/localai-cowork-server-linux-x64.tar.gz` enthält einen
nativen Linux-x64-Daemon, Webfrontend und Startdateien. Die minimale
Laufzeitbasis dieser Lieferung ist Ubuntu 24.04 mit glibc 2.39. Eine
Kompatibilität mit älteren Distributionen wird hier nicht bestätigt; dafür
ist ein eigener nativer Build erforderlich.

Tar in ein eigenes neues Verzeichnis entpacken und dort prüfen und installieren:

```bash
sha256sum --check SHA256SUMS
./install-user.sh
~/.local/share/localai-cowork-v2/server-bundle/start.sh
```

Der Installer kopiert in ein neues Ziel und richtet keinen Systemdienst ein.
Der Vordergrundstart verwendet SQLite und einen normalen Worker; Ctrl-C beendet
diesen eigenen Vordergrunddaemon. Standarddaten liegen unter
`~/.local/share/localai-cowork-v2/server-data`, die API auf
`127.0.0.1:43821`. `COWORK_DATA_DIR` kann einen anderen privaten absoluten
Datenordner vorgeben. Ein bewusst ausgewähltes `COWORK_DATABASE_URL` ermöglicht
PostgreSQL; dadurch entstehen keine zusätzlichen Teamrechte.

Für einen verwalteten Host `nginx.conf.example` an Domain und vorhandenes
TLS-Zertifikat anpassen und
`COWORK_PUBLIC_ORIGIN=https://eigene-domain.example` beim Daemon setzen.
Dieser Wert ist ein exakter HTTPS-Origin ohne Pfad oder Wildcard. Der Proxy
leitet zum lokalen Daemon weiter. `cowork-v2.service.example` ist eine
anzupassende Vorlage für einen vom Benutzer ausdrücklich eingerichteten Dienst.

Im Client **Eigener V2-Server** mit der HTTPS-Adresse wählen. Teammitglieder
verwenden **Mit Teamkonto anmelden**. Der persönliche Betreiberzugang verwendet
separat den privaten `daemon.token` dieses Servers. `native.token` bleibt
am Host und gehört niemals in Browser, URL oder Client-Verbindungseingabe.
Ein SSH-Tunnel kann stattdessen einen eigenen Loopback-Endpunkt bereitstellen:

```bash
ssh -N -L 43821:127.0.0.1:43821 user@server
```

Dann `http://127.0.0.1:43821` verwenden. Eine solche Serververbindung erteilt
keine lokale native Ordner- oder Schlüsselautorität. Zum erneuten lokalen
Desktopbetrieb **Lokalen Arbeitsplatz öffnen** wählen. Daten liegen jeweils
auf dem ausgewählten System; der Verbindungswechsel verschiebt sie nicht.

## Compose, Kubernetes, Dateien und Backups

Für einen eigenen Linux-Compose-Host aus den entpackten vollständigen Quellen:

```bash
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork init --site cowork.example.com
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork build
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork start
python3 deploy/v2/lifecycle.py --root /srv/localai-cowork status
```

Das neue Ziel muss privat und unbenutzt sein. DNS, erreichbare Ports 80/443 und
Docker Compose sind Betreiberinputs. Die Standardtopologie verwendet eine API,
einen Worker, PostgreSQL und TLS-Proxy. API und Worker teilen die gleiche private
Daten-/Schlüsselablage. Teamwerkzeuge werden separat mit
`configure-team-tools` eingerichtet; nur der vertrauenswürdige Worker erhält
den Docker-Socket. Die ausführlichen Image-, Workspace-, UID- und Netzwerkregeln
stehen in [deploy/v2/README.md](../deploy/v2/README.md).

Ohne S3-Konfiguration verwenden Teamdateien den verwalteten lokalen Speicher.
Externes S3 verlangt Endpoint, Bucket, Region und ausdrücklich gewählte private
Credential-Dateien bzw. Betreiber-Umgebungsvariablen. Standard ist HTTPS.
Die optionale lokale MinIO-Topologie wird mit `init --local-s3` bewusst gewählt;
sie verwendet für ihren internen Endpoint ausdrücklich
`COWORK_S3_ALLOW_INSECURE_HTTP=1`. Das ist keine automatische Freigabe beliebiger
HTTP-Hosts. Ein S3-Backend wird durch diese Dokumentation nicht betrieben oder
abgenommen.

Vorlagen für Kubernetes und HA benötigen ein vorhandenes Cluster, Registry,
Storage, TLS und ausdrücklich ausgewählte Betreiberkonfiguration. Gemeinsam
genutzte Workspaces müssen auch auf den Docker-Hosts physisch dieselbe Ablage
sein. Die Compose-Lebenszyklusbefehle verwalten keine Kubernetes-Ressourcen.

Backup/Restore/Update/Rollback sind ausdrückliche Betreiberbefehle. Das
Compose-Backup stoppt die eigenen Writer, erstellt einen logischen
PostgreSQL-Dump und sichert private Daten/Schlüssel, gegebenenfalls lokale
MinIO-Bytes, als `age`-verschlüsseltes Archiv. Externe S3-Objekte benötigen eine
zusätzliche Objektbackupstrategie. Ganze private Master-Schlüsselablage und
Datenbank zusammen erhalten; ohne passende Schlüssel sind gespeicherte
Teamcredentials nicht entschlüsselbar. V2-Inhalts-ZIPs ersetzen dieses Backup
nicht. Alte Binaries nicht gegen ein neueres Schema starten; Rollback setzt
einen bewusst gewählten Datenbank-/Schlüsselcheckpoint voraus und kann spätere
Schreibvorgänge verlieren. Details stehen in der Deploymentanleitung.

## Alte V1-Inhalte ausdrücklich übernehmen

Zuerst einen ausdrücklich ausgewählten, kalten Export bereitstellen; keinen
laufenden V1-Dienst oder seine Datenbank als V2-Datenspeicher verwenden.
Im lokalen Desktop unter **Import & Export → V1 übernehmen** die Quellfamilie
**V1-Desktop** oder **V1-Daemon / Serverexport** wählen. Bei Originalanhängen die
entsprechende Option aktivieren. Der erste Picker wählt die JSON-/SQLite-Datei,
der zweite ihren Original-Anhangordner. Vorschau, Herkunft und fehlende Anhänge
prüfen, dann den gespeicherten Plan ausdrücklich übernehmen.

Für ein Team zuerst ein aktives Zielprojekt mit eigenen Schreibrechten wählen
und unter **V1-Import** **Datei und Anhangordner auswählen** verwenden. Alle
ausgewählten Legacyprojekte werden als neue Aufträge in dieses Teamprojekt
übernommen; der Plan nennt die Zuordnungen. Erst **Diesen Plan ausdrücklich in
das Teamprojekt übernehmen** bestätigt den Import. Dieser native Quellweg
gehört zum lokalen Desktop-Daemon. Übernommene Teaminhalte sind danach mit
aktuellen Rechten auch im Web lesbar.

Originalkennungen bleiben Herkunft, alte Läufe werden nicht gestartet oder
fortgesetzt. Anhänge außerhalb des ausgewählten Roots, nicht unterstützte
Medien und fehlende Dateien erscheinen als begrenzter Importbefund. Die
[Featurebeschreibung](FEATURES-ALPHA3.md) nennt Größen- und Aufnahmegrenzen.
Eine neue Android-App, automatische Live-V1-Migration und vollständige
V1-/M0–M10-Parität werden nicht durch die Installation bereitgestellt.
