# Installieren und demonstrieren

Der persönliche V2-Arbeitsplatz kann mit einem gebündelten lokalen Daemon oder
mit einem später selbst aufgesetzten V2-Server betrieben werden. Die aktuelle
Version ist `2.0.0-alpha.7` (maßgeblich: `package.json` und die Build-Metadaten des jeweiligen Pakets). Die vollständige Umsetzung aus M0–M10 und die
gemeinsame Abnahme mit dem bestehenden Open-Cowork-Server stehen noch aus.

## Desktop ohne externen Server

Der Windows-Installer und die Linux-Pakete enthalten die Desktop-Anwendung und
ihren passenden Daemon. Die Anwendung öffnet den privaten lokalen Arbeitsplatz
automatisch; im Verbindungsdialog ist er auch über **Lokalen Arbeitsplatz öffnen**
erreichbar. Projekte, Aufträge und die Datenbank werden im eigenen V2-Datenordner
gespeichert. V1-Datenbanken dürfen nicht als V2-Datenbank verwendet werden.

Die Linux-Desktoppakete benötigen glibc mindestens 2.39 und zielen auf
Ubuntu 24.04 mit GTK3 und WebKitGTK 4.1. Das Debian-Paket deklariert diese
glibc-Mindestversion zusätzlich zu seinen GTK-/WebKit-Abhängigkeiten.
Ein AppImage hebt die glibc-Mindestversion nicht auf. Für Windows wird
WebView2 benötigt. Eine Abnahme anderer Linux-Distributionen steht aus.

Ohne FUSE kann das AppImage mit `--appimage-extract-and-run` gestartet werden.
Dieser Startweg und das normale Schließen seines Fensters wurden geprüft.

Für einen lokalen Modelllauf muss ein Modellanbieter bereits installiert sein.
Die Anwendung lädt keine Modelle herunter. Der geprüfte Ablauf mit dem bereits
vorhandenen Ollama-Modell `qwen3:4b` ist:

1. Projekt und Auftrag anlegen.
2. **Modelle einrichten** öffnen, Ollama und `http://127.0.0.1:11434` auswählen,
   einen Namen und `qwen3:4b` eintragen und verbinden.
3. Das neue Modell im Modell-Auswahlfeld des Auftrags ausdrücklich auswählen.
4. Beispielsweise `Antworte ausschließlich mit dem Wort Bereit. Verwende keine
   Werkzeuge. /no_think` eingeben und den Lauf starten.
5. Antwort und Laufzustand prüfen. Bestätigte Daten bleiben nach einem
   Daemon-Neustart erhalten.

**Geldbudget** zeigt den dauerhaft gespeicherten gemeinsamen Rahmen an.
Ohne Betreiberkonfiguration wird dies ausdrücklich angezeigt; es wird keine
fiktive Geldgrenze eingerichtet. Die Einrichtung und Bindung tatsächlicher
Preise und Deployments beschreibt [die Geldkonfiguration](PERSONAL-MONEY-CONFIGURATION.md).
Token- und Laufgrenzen bleiben zusätzlich wirksam.

Native Ordner- und Schlüsselaktionen gehören zum lokalen Desktop-Arbeitsplatz.
Eine manuell gewählte Serververbindung erhält diese Rechte auch dann nicht,
wenn sie über einen Loopback- oder SSH-Tunnel erreichbar ist. Verbindungen und
Bearer werden zusammen nur für die aktuelle Browsersitzung gespeichert.

## Eigenen V2-Server später installieren

Das Linux-x64-Serverarchiv enthält den nativen Release-Daemon, das Webfrontend,
Prüfsummen, `install-user.sh`, `start.sh` sowie systemd- und nginx-Vorlagen.
Der enthaltene Daemon benötigt x86_64, glibc mindestens 2.38 und
`libgcc_s.so.1`. Geprüft wurde Ubuntu 24.04 mit glibc 2.39 unter WSL2.
Für ältere Systeme muss ein passender nativer Daemon aus den Quellen gebaut
und ein neues Serverpaket erzeugt werden.
Die [Serveranleitung](../tools/server/README.md) beschreibt Installation,
Vordergrundstart, private Token-Dateien, SSH-Tunnel und HTTPS.

Nach dem Start im Browser die Serveradresse öffnen und den privaten
`daemon.token` in das Passwortfeld eingeben. In der Desktop-App kann dieselbe
kompatible V2-Adresse im Verbindungsdialog ausgewählt werden. Für Fernzugriff
wird HTTPS verwendet; unverschlüsseltes HTTP ist auf die unterstützten
Loopback-Hosts `127.0.0.1` und `localhost` beschränkt. Token gehören nicht in URLs.

Der Server betreibt derzeit den persönlichen Arbeitsplatz mit einem privaten
Bearer. Die Organisations- und Ausführungsbrücke zum bestehenden V1/Open-Cowork
ist noch keine gemeinsam abgenommene Funktion.

Einige Projekt-, Lauf- und Dateidetailtexte verwenden in dieser Alpha noch
lokale Formulierungen, auch bei einer Serververbindung. Projekte, Dateien und
Modellläufe liegen dann auf dem ausgewählten Server. Die Serveradresse im
Verbindungsdialog ist für den tatsächlichen Ausführungsort maßgeblich; die
durchgängige Anpassung dieser Detailanzeigen bleibt offen.

## Pakete aus den Quellen erzeugen

Benötigt werden die Rust-Toolchain aus `rust-toolchain.toml`, Node/npm und die
jeweiligen nativen Plattformabhängigkeiten. Windows benötigt MSVC und WebView2;
Linux benötigt GTK3 und WebKitGTK 4.1. Der Repository-Lock pinnt Tauri CLI 2.11.4.
Windows-Pakete werden unter Windows, Linux-Pakete unter Linux erzeugt.

Nach `npm ci` und `npm run build` das übergeordnete Verzeichnis `.local` anlegen.
Die mit `--output-dir` gewählten Ausgabeverzeichnisse müssen für jeden Versuch
neu sein. Der mit `--target-dir` gewählte Buildcache darf wiederverwendet werden:

```text
python tools/desktop/release.py --output-dir .local/sidecar --target-dir .local/native-build
python tools/desktop/package.py --sidecar-dir .local/sidecar --output-dir .local/desktop-packages --target-dir .local/native-build --bundles nsis
```

Unter Linux beim zweiten Kommando `--bundles deb,appimage` verwenden.
Das Ausgabeverzeichnis muss auf einem nativen Linux-Dateisystem liegen.
Insbesondere unterstützt WSL-DrvFS unter `/mnt/c` die für diesen Paketierer
erforderliche exklusive atomare Veröffentlichung nicht; dort wird der
Paketabschluss abgewiesen. Unter WSL beispielsweise einen neuen Ausgabepfad
unter dem eigenen Linux-Home verwenden.
`--offline` ist möglich, wenn alle exakt gelockten Abhängigkeiten bereits
vorliegen. Der Paketierer prüft die Source-/Sidecar-/Frontend-Bindung, führt
native Desktop-Tests sowie Format und Clippy aus und baut die Pakete in einem
getrennten Quellkontext. Alte Sidecars und Paketverzeichnisse werden nicht ersetzt.

Das Serverpaket entsteht aus dem nativen Linux-Sidecar:

```text
python3 tools/server/package.py --prepared .local/sidecar --web-dist apps/web/dist --output .local/server-bundle --archive .local/server-bundle.tar.gz
```

Die erzeugten Pakete und Quittungen sind für eine spätere Installation
vorbereitet. Signierung, Veröffentlichung, automatische Updates sowie die
vollständige Release- und Installationsabnahme bleiben eigene offene Schritte.
Den detaillierten Pflichtumfang führt [der vollständige Arbeitsplan](COMPLETION-WORKPLAN.md).
