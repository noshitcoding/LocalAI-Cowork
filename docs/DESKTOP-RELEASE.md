# Native Release-Sidecar vorbereiten

**Quellstand 12. Oktober:** Signier-/SBOM-Werkzeuge und der native Updateweg sind
ergänzt. Der [neue Recoveryweg](DESKTOP-UPDATE-RECOVERY.md) erhält ursprüngliche
Updatevorgänge auch nach GUI-Verlust und bindet den physischen Daemonstop an ein
übertragenes OS-Handle/PIDfd. Der unabhängige Windows-Installerbeobachter und
der gebundene NSIS-Paketvertrag sind im Code ergänzt. Neue Gesamtpakete und die
aktuelle physische Installations-/Releaseabnahme bleiben offen. Die nachfolgenden
früheren Belege werden dadurch nicht nachträglich erweitert.

Die Paketierung prüft jetzt auch die Ressourcen des tatsächlich erzeugten
Windows-NSIS-Pakets: `CoworkInstallerContract=nsis.currentUser.originalProcess.v1`
und die exakte `ProductVersion` müssen in allen Ressourcentabellen übereinstimmen.
Fehlende Marker oder eine abweichende Version verhindern die Paketveröffentlichung.
Der Paketbeleg erhält `native_installer_contract` mit Paket-SHA256 und geprüften
Tabellen. Die kleine Probe mit einem tatsächlich kompilierten NSIS-Hookpaket
besteht und weist eine falsche Zielversion zurück; sie führt keinen Installer aus.

Die SPDX-2.3-Stückliste erfasst beide originalen Cargo-Lockgraphen, den npm-
Lockgraphen, Paketbytes und die verwendeten Desktop-/Daemon-Payloads. PE-/ELF-
Importtabellen werden ohne Programmausführung gelesen. Externe OS-Bibliotheken
bleiben Anforderungen, keine erfundenen installierten Komponenten. Die
Quellgraphen enthalten auch Build-, Dev- und andere Zielplattformabhängigkeiten;
eine Verknüpfung sämtlicher Quellpakete mit jedem Binary wird nicht behauptet.

Die private Werkzeuginstallation schreibt vor Ready eine eigene unveränderlich
benannte `tool-runtime-sbom-*.spdx.json`: tatsächliche Softwaredateien, geprüfte
Python-METADATA/RECORD-Dateien, Paketversionen, native Importe und ausdrücklich
ausgewählte externe Programmbytes. Der private Setup-Beleg Format 2 bindet deren
SHA256. Benutzerdateien, Zugangsdaten und Browserprofile werden nicht ausgewählt.
Originale Upstream-SPDX-Dokumente für Fremdbinaries können mit dem exakten
Payloadhash angehängt werden; nicht passend gebundene Dokumente werden abgewiesen.
Die Stückliste ersetzt keine Signierung, Lizenzfreigabe oder Releaseabnahme.

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

## Desktop-Installer erzeugen

`tools/desktop/package.py` baut aus einer aktuellen Release-Sidecar-Quittung
die native Desktop-Anwendung und anschließend NSIS unter Windows beziehungsweise
Debian und AppImage unter Linux. Tauri CLI ist mit Version `2.11.4` im
Workspace-Lock gebunden. Der Helfer verwendet einen neuen Quellkontext außerhalb
des ursprünglichen Cargo-Workspaces, prüft sämtliche Core-, Desktop-, Frontend-,
CLI- und Sidecarbytes und veröffentlicht ein neues Paketverzeichnis.
Native Desktop-Tests, Format und Clippy laufen im selben Targetcache vor dem
Paketbau. Die [Installationsanleitung](DEMO-AND-INSTALLATION.md) enthält die Befehle.
Linux-Ausgabepakete benötigen ein natives Linux-Dateisystem mit exklusivem
atomarem Rename; ein WSL-DrvFS-Ausgabepfad unter `/mnt/c` wird abgewiesen.

Der lokale Desktop verbindet sich nur mit einem passenden Healthvertrag
(Produkt, Version, Ereignisschema, Deployment und Geldstatus-Capability).
Der feste Loopback-Readinessaufruf verwendet weder Umgebungsproxy noch Redirects.
Manuell ausgewählte Serververbindungen erteilen keine lokale Ordner- oder
Schlüsselautorität. Diese Produktänderungen sind gegenüber den unten erhaltenen
älteren Sidecarbelegen neu und benötigen ihre eigenen aktuellen Prüfungen.

## Release-Sidecar

`tools/desktop/prepare.py` bleibt der Entwicklungsweg. Für Releasebytes dient
`tools/desktop/release.py`. Das Werkzeug baut den nativen Windows- oder
Ubuntu-x64-Daemon mit `cargo build --locked --release`, wählt ausschließlich
das von diesem erfolgreichen Build gemeldete ausführbare Artefakt und prüft
Releaseprofil, genaue Targetposition sowie PE32+/ELF-Architektur.

```powershell
python tools/desktop/release.py --offline `
  --output-dir .local/my-release-sidecar `
  --target-dir .local/my-release-build
```

Der Ausgabepfad muss neu sein; sein Elternverzeichnis muss existieren. Ein
fehlgeschlagener Build ersetzt keine frühere Vorbereitung. Das neue Verzeichnis
enthält die Binärdatei mit dem Tauri-Targetsuffix, die Cargo-JSONausgabe und
`receipt.json`. Die Quittung bindet Toolchain, Kommando, Package-ID/-Version,
Binärgröße/-SHA256 und die unveränderten Coreeingaben vor und nach dem Build.
Abweichende Compilerwrapper, Buildflags und Releaseprofil-Umgebungsvariablen
benötigen einen separat geprüften Buildweg und werden hier abgewiesen.

Elf Gegenbeispiele prüfen unter anderem Debug-/Fremd-/mehrdeutige Artefakte,
einen alten Binaryrest nach fehlgeschlagenem Build, falsche Architektur,
die unabhängige Kopie eines regulär von Cargo erzeugten Hardlinkartefakts sowie
Änderungen der Quellen oder Binärdatei während der Vorbereitung. Alle eingebetteten
historischen SQLite-/JSON-Seeds und synthetischen Verifierfixtures werden auch
ohne Quelltext-Dateisuffix gebunden; eine Seedänderung verwirft die Veröffentlichung.
Der erste versiegelte native Windowsnachweis (private development evidence; omitted from public sources)
bindet 221 Eingaben und besteht diese neun Fälle sowie einen echten warmen
Releasebuild. Quittungs-SHA256:
`15bc13d489bf2e6089ff2377cb969a35874fb73847c711b7d7707dafff2bb365`.
Die vorbereitete EXE ist 39.183.872 Bytes groß; SHA256
`530523ba10ce0c3bc22f0a42157c4bf93cc22ee3754e9023f18315b9ab7e4fae`.
Alle 15 eigenen Jobprozesse endeten natürlich; retained Native-Handles
bestätigen den Exit. Der erhaltene erste Fehlversuch hatte eine zu strenge
Hardlinkprüfung und einen nach Cleanupfrist ausschließlich im eigenen Job
beendeten MSVC-vctip-Prozess. Seine Quittung zählt nicht als Pass. Der
Quellenstand bleibt historisch, sobald die folgende Operatorintegration ihn
ändert. Die vorbereitete EXE wurde in dieser Prüfung nicht ausgeführt.

Der historische Windowsbeleg (private development evidence; omitted from public sources)
bindet Schema21-Core222 plus Helper/Test, insgesamt 224 unveränderte Eingaben.
Neun Gegenbeispiele und der tatsächliche warme native Releasebuild bestehen.
Quittungs-SHA256: `071a9a15050154cb5554a93c2ce49113abf7fc6a2116e53e600e1f1a8f711525`.
Die neue EXE hat 39.654.912 Bytes und SHA256
`aab05f6b6d9c644cb47cc9629da89971c468105e98491bf5917d8d4055bd486c`.
Alle 15 eigenen Jobprozesse enden natürlich mit positiven nativen Waits.
Die erhaltenen jüngeren Fehlversuche wegen vctip-Cleanup und eines temporären
Speicherplatzfehlers werden dadurch nicht zu bestandenen Prüfungen.

Der historische native Linuxbeleg (private development evidence; omitted from public sources)
bindet dieselben 224 Eingaben. Neun Gegenbeispiele und ein tatsächlicher
`--locked --offline --release`-Build bestehen auf einem eigenen warmen ext4-Cache.
Quittungs-SHA256: `9f50aad01c8ccf99fc3f91e8a0c654ba6782b2654c2909fdbe8ddd82ef40130a`.
Das ELF64-Artefakt hat 43.249.672 Bytes und SHA256
`5bb57c9d1ea130be8c8ec0d2bc3810ba85ff1e2933eac4c0a9e5cdc41d2afd89`.
Beide eigenen Linuxsessions sind leer, alle gehaltenen PIDfds signalisieren
Exit, und alle sechs Prozesse des äußeren Windowsjobs enden natürlich.
Der erste Linuxbuild verweigerte wegen einer zwischenzeitlichen Testquelländerung
die Artefaktveröffentlichung. Dieser Fehlbeleg und der inzwischen historische
erste Warmbeleg bleiben erhalten. Die neuen Releaseartefakte wurden in diesen
Vorbereitungsprüfungen nicht ausgeführt.

Die Vorbereitung baut und prüft keinen Installer. Sie signiert, installiert
oder veröffentlicht nichts. Native Tauri-Paketierung, Linux-/Windows-
Installationsläufe, SBOM, Signatur-/Updatekette und die vollständige M1-/M10-
Releaseabnahme bleiben erforderlich. Die Quittung bezeichnet diese Grenzen
mit `installer_built`, `signed`, `installed` und `release_qualified: false`.
