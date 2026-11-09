# Persönlicher V2-Server ohne Docker

Dieses Paket enthält den nativen Linux-x64-Daemon, das Webfrontend und
Start-/Installationsdateien für die spätere eigene Installation. Der aktuelle Stand enthält
eigene Teamkonten, Inhaltsrechte, teamweise verschlüsselte LiteLLM-Schlüssel,
Teamaufträge und Workflows. Normale Mitglieder wählen im Verbindungsdialog
**Mit Teamkonto anmelden** und benötigen keinen Betreiber-Bearer.
Siehe [neue Funktionen und Einrichtung](../../docs/FEATURES-ALPHA3.md) sowie
[Compose/Kubernetes und Lebenszyklus](../../deploy/v2/README.md).
Signaturen und die Produktionsabnahme sind nicht Bestandteil des schnellen
Paketbaus. Ein bestehender V1-/Open-Cowork-Tenantendpunkt ist weiterhin ein
anderer Serververtrag.

Der mitgelieferte ELF-Daemon benötigt Linux **x86_64 und glibc mindestens 2.38**
sowie `libgcc_s.so.1` und die OpenSSL-3-Systembibliotheken (`libssl.so.3`,
`libcrypto.so.3`; unter Ubuntu 24.04 Paket `libssl3t64`). Frühere Alpha.1-Paketbytes wurden mit normalem Worker, geordnetem
Stop und Wiederstart unter **Ubuntu 24.04 mit glibc 2.39 in WSL2** geprüft.
Für Alpha.4 wird auf Wunsch nur gebaut; diese historischen Starts bestätigen
nicht den neuen Funktionsstand.
Andere Distributionen sind damit nicht separat abgenommen. Auf einem älteren
glibc-Host muss der Daemon aus den Quellen nativ auf diesem eigenen Host gebaut
und anschließend neu paketiert werden; der gelieferte ELF ist dort nicht direkt
ausführbar. Dafür wird kein Docker benötigt.

## Paket erstellen

Der aktuelle Releasehelfer baut den nativen Daemon aus den vorhandenen Quellen:

```bash
python3 tools/desktop/release.py --output-dir /absolute/new/prepared \
  --target-dir /absolute/existing/cargo-cache
npm ci
npm run build
python3 tools/server/package.py --prepared /absolute/new/prepared \
  --web-dist apps/web/dist --output /absolute/new/server-bundle
```

Rust verwendet die Version aus `rust-toolchain.toml`; das Frontend benötigt
Node/npm. Das Paketieren prüft den tatsächlichen ELF-Daemon und dessen
Release-Quellenquittung erneut. `SHA256SUMS` bindet die gelieferten Dateien.
Quellbindung und Prüfsummen sind keine Herausgebersignatur.

Das Verzeichnis `build-source` enthält den an dieselbe Daemonquittung gebundenen
Buildkontext einschließlich der Rust- und Frontendquellen, Compose-/Kubernetes-
Dateien, Werkzeug-Bridges, Image-Rezepte und Einrichtungshelfer. Seine
`BUILD-SOURCE-RECEIPT.json` bindet jede Originaldatei mit Größe und SHA-256;
die äußeren `SHA256SUMS` binden auch diese Quittung. Für eine neue eigene
Composeinstallation kann `build-source/deploy/v2/lifecycle.py` verwendet werden.
Der Helfer setzt diesen vollständigen Buildkontext als `COWORK_SOURCE_ROOT`.
Die dortige `deploy/v2/README.md` beschreibt Einrichtung, gVisor, S3, Sicherung
und Lebenszyklus. Python-, Docker-, Kubernetes- und Werkzeugabhängigkeiten
werden auf dem eigenen Host entsprechend dieser Anleitung eingerichtet.
CrewAI-Wheels und ihre aus den tatsächlichen Wheels erzeugte Lockdatei werden
vor dem Crew-Imagebau mit `build-source/tools/private_crew_setup.py` vorbereitet.
Das Paket enthält keine Betreiber-Credentials oder eingerichteten Dienste.

## Auf dem eigenen Linux-x64-Host starten

```bash
sha256sum --check SHA256SUMS
./install-user.sh
~/.local/share/localai-cowork-v2/server-bundle/start.sh
```

Der Vordergrundstart enthält den normalen Worker. Ctrl-C stoppt API und Worker.
Standardmäßig bindet die API ausschließlich `127.0.0.1:43821` und legt die neue
SQLite-Datenbank unter `~/.local/share/localai-cowork-v2/server-data` an. Eine
bereits vorhandene Installation wird vom Installer abgewiesen. Updates müssen
in ein neues Verzeichnis; Datenbankkopien nur bei gestopptem Daemon erstellen.
V1-Datenbanken niemals als V2-Datenbank öffnen.

Beim ersten Start entstehen zwei private Autoritätsdateien im Datenverzeichnis.
Für die Browser-/Desktopverbindung wird ausschließlich `daemon.token` verwendet.
`native.token` bleibt am Host und wird weder in den Browser noch in die
Verbindungsanzeige kopiert. Bestehende Token-Dateien und Verzeichnisse müssen
ihre geprüften privaten Besitz-/Rechtebedingungen erfüllen; der Daemon repariert
unsichere Dateien nicht. Token nie in URLs, Kommandoargumente oder Logs schreiben.

Für Zugriff ohne öffentlichen Reverseproxy:

```bash
ssh -N -L 43821:127.0.0.1:43821 user@server
```

Danach `http://127.0.0.1:43821` im Browser öffnen oder im V2-Client den eigenen
Server mit diesem Loopback-Endpunkt auswählen. Den privaten Bearer in der
Verbindungseingabe verwenden. Desktop-Dateizugriff/Schlüsseldienste auf einem
entfernten Host erfordern weiterhin deren eigene qualifizierte native Autorität.

## HTTPS und optionaler Benutzer-Service

`nginx.conf.example` ist eine Vorlage für einen bereits administrierten Host
mit echtem TLS-Zertifikat. Domain/Zertifikatpfade anpassen und beim Daemon
`COWORK_PUBLIC_ORIGIN=https://eigene-domain.example` setzen. Der Wert ist ein
exakter kanonischer HTTPS-Origin ohne Pfad, abschließenden Slash oder Wildcard;
ein abweichender Browser-Origin wird abgewiesen. Der Browser kann denselben
HTTPS-Origin bedienen, während der Daemon weiter nur auf Loopback hört.

`cowork-v2.service.example` kann nach Anpassung als
`~/.config/systemd/user/cowork-v2.service` verwendet werden. Danach kann der
Benutzer selbst `systemctl --user daemon-reload` und
`systemctl --user enable --now cowork-v2` ausführen. Der Installer führt diese
Schritte nicht aus. Ein Benutzer-Service nach Abmeldung benötigt eine vom
Hostadministrator bewusst eingerichtete Benutzer-Lifetime.

## Provider und Geldkonfiguration

Ohne eingerichteten Provider führt der normale Worker keine Modellaufrufe aus.
`providers.json` im privaten Datenverzeichnis enthält die tatsächlichen
`ConfiguredProvider`-Profile einschließlich der Namen benötigter
Credential-Umgebungsvariablen. Credentials gehören nicht in das Paket.

Eine eigene operatorseitige Geldkonfiguration kann beim ersten Start über
`COWORK_PERSONAL_MONEY_CONFIG_FILE` aktiviert werden. Die Datei muss im privaten
Datenverzeichnis liegen und die strikten Besitz-/Rechte-/Formatprüfungen bestehen.
Preis- und Modellbindungen müssen den tatsächlich verwendeten Deploymentdaten
entsprechen; dieses Paket liefert keine Preise oder Token-/Modellparität.
Die Aktivierung bleibt dauerhaft und unveränderlich; das Entfernen der
Umgebungsvariable deaktiviert sie nicht. Die Geldbudgetanzeige ist ausschließlich
lesend und erteilt kein Ausführungs- oder Erhöhungsrecht.
