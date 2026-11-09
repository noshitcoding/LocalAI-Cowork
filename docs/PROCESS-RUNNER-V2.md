# V2-Prozessrunner für isolierte persönliche Abläufe

Der eigenständige Receiver verwendet ausschließlich das V2-Runnerprotokoll. Er
braucht weder einen V1-Daemon noch V1-IPC. `cowork-process-runner` überwacht die
physischen Prozesse und hält ihre Belege unabhängig von einer HTTP-Verbindung.
`cowork-runner-probe` ist ein separater, im Release enthaltener Prüfprozess.

## Native Pakete vorbereiten

Python 3.11 oder neuer ist für die Einrichtung erforderlich. Das Paket enthält
die beiden nativen x64-Programme, den Konfigurator, private Windows-Dateihilfen,
Lizenz, diese Anleitung und ein SHA256-Dateimanifest. Es enthält keine Schlüssel.

Unter Windows oder Linux auf dem jeweiligen nativen Buildhost:

```text
python tools/process_runner/package.py --build --cargo ABSOLUTER_CARGO_PFAD --output ABSOLUTER_NEUER_ARCHIVPFAD
```

Windows verwendet `.zip`, Linux `.tar.gz`. Der Build ist `--locked --offline
--release`; beide exakten Cargo-Artefakte werden unabhängig kopiert. Alternativ
lassen sich bereits überprüfte, nicht hartverknüpfte Programme mit `--receiver`
und `--probe` auswählen. Das Manifest unterscheidet Releasebuild und gelieferte
Programme. Verpackung behauptet keine physische Qualifizierung oder Signatur.
Veröffentlichen und Signieren des Archivs sind separate Releaseoperationen.

Aktuelle Pakete enthalten `sbom.spdx.json` mit gesperrtem Quellgraph,
Binary-SHA256 und tatsächlichen nativen Bibliotheksimporten. Bei gelieferten
Programmen ist `--source-sbom ABSOLUTER_ORIGINAL_SPDX_PFAD` erforderlich: Seine
Dateiprüfsummen müssen beide gelieferten Programme binden. Das Originaldokument
wird unverändert beigelegt. Aktuelle Workspacequellen werden dabei nicht als
Quellen fremder bereits gebauter Programme ausgegeben.

`rustup which cargo` nennt das tatsächlich installierte Cargo der Projekttoolchain.
Der Builder benötigt dieses gewöhnliche Programm und seine benachbarten `rustc`-
und `rustdoc`-Programme; Rustup-Proxys/Symlinks werden nicht als Buildprogramme
übernommen. Er bindet die Compiler absolut, ohne einen Channel zu installieren.
Das Release-Manifest enthält außerdem die SHA256-Werte der tatsächlichen
Buildquellen und des gemeinsamen Lockfiles. Eine Quellenänderung während des
Builds bricht die Paketierung ab.

## Gemeinsame Voraussetzungen

- Dedizierter lokaler Betreiber und private, nicht synchronisierte Datenordner.
- Derselbe Betriebssystembenutzer wie der Daemon/FileBroker; der Receiver muss
  dessen exaktes Workspace-State-Verzeichnis und OS-Ressourcensperren nutzen.
- PEM-Zertifikatskette mit passendem Hostnamen und zugehöriger PKCS8-TLS-Key.
  Der Konfigurator kopiert beide privat; HTTPS und Zertifikatsprüfung bleiben aktiv.
- Ausreichend freier Speicher für den ausdrücklich gewählten Scratchbereich,
  Snapshots und Retention. Große Ressourcenlimits müssen zum Betreibergerät passen.

Entpacktes natives Paket einrichten, ohne Dienste zu starten:

```text
python configure.py --output ABSOLUTER_NEUER_PRIVATER_ORDNER --workspace-state ABSOLUTER_FILEBROKER_STATE --tls-cert ABSOLUTE_KETTE_PEM --tls-key ABSOLUTER_PKCS8_KEY_PEM --public-origin https://receiver.example:7439 --listen 127.0.0.1:7439
```

Alle Pfade müssen absolut, gewöhnlich und lokal sein. Symlinks, Reparse Points
und hartverknüpfte Eingabedateien werden abgewiesen. Das Ziel muss neu sein.
Der Konfigurator importiert keine Team-/Modellschlüssel. Es werden neue private
Kontroll- und Runnersignaturschlüssel sowie ein Transporttoken erzeugt.

Der Daemon erhält ausschließlich den privaten Pfad:

```text
COWORK_PROCESS_RUNNER_CONFIG=INSTALLATIONSORDNER/state/daemon-runner.json
```

Direkter, ausdrücklich gewählter Start:

```text
INSTALLATIONSORDNER/bin/cowork-process-runner serve INSTALLATIONSORDNER/state/runner.json
```

Unter Windows endet der Programmname auf `.exe`.

## Windows: echtes LPAC, Job und VHD

Windows 10/11 beziehungsweise ein unterstütztes Windows Server mit LPAC und
VHD-Provider ist erforderlich. Der Betreiber benötigt administrative Rechte
für feste VHDs und die dauerhafte Volume-Flushing-Barriere. Falls die tatsächlich
erforderlichen Rechte/Mechanismen fehlen, verweigert die Qualifizierung den Start.

Beim Konfigurieren `--windows-bundle ABSOLUTER_PAKETORDNER` ergänzen; wiederholbar
für bis zu 32 Pakete. Vollständige Laufzeiten einschließlich ihrer DLLs/Assets
werden privat kopiert, mit Datei- und Verzeichnismanifest gehasht und nach diesem
Digest registriert. Es gelten maximal 10.000 Einträge, 64 MiB pro Datei und
512 MiB pro Paket. Programmaliase beziehen sich auf vorhandene EXEs direkt an der
Paketwurzel, beispielsweise `python=python.exe`; beliebige Hostprogramme sind
keine Ausweichlösung. Der Konfigurator nennt den Manifestdigest für die UI.

Das UI unter „Umgebungen“ legt die Version mit diesem Bundle-SHA, Aliasen und
Ressourcenlimits an. Windows unterstützt maximal 64 Prozesse, 64 GiB Jobmemory
und einen Scratchbereich ab 64 MiB in ganzen MiB. Jede Ausführung erhält eine
frische feste NTFS-VHD, LPAC-Identität und einen eigenen Job. Netzwerk, Hostdateien
und Betreibercredentials werden nicht an den Prozess weitergereicht.

Für den dauerhaften Receiver den erzeugten `install-service.ps1` prüfen und
explizit als derselbe Betreiber mit administrativen Rechten ausführen. Er legt
eine neue Aufgabe mit Start beim Systemboot, S4U-Betreiberidentität und höchsten
Betreiberrechten an. Bestehende Aufgaben werden nicht überschrieben. Die Aufgabe
startet nur den Receiver; Kinder erhalten weiterhin LPAC-/Jobrechte. S4U und
Batchanmeldung müssen auf dem Gerät durch die Windows-Richtlinien erlaubt sein.
`uninstall-service.ps1` entfernt ausschließlich diese Aufgabe und behält private
Schlüssel und Belege. Ein Stop/Crash verliert die Jobhandles; der Job beendet
seine Kinder, und der nächste Receiverstart klärt eigene verbliebene Ressourcen.

## Windows: administrative Orphan-Klärung

Der Quellenstand vom 12. Oktober ergänzt den Fall, dass ein Receiver nach
AppContainer-Erstellung vor dem ersten `owned`-Beleg endet. Der lokale
Betreiber hält den Receiver an und verwendet dessen ursprüngliche Konfiguration,
den genauen Attempt und die ursprüngliche Capsule auf der weiterhin bestätigten
VHD. Es gibt keine HTTP-Löschroute und keinen Scan, der Profile nach Präfix löscht.

```powershell
cowork-process-runner.exe orphan inspect C:\CoworkV2\state\runner.json C:\CoworkV2\state\attempts\ORIGINAL_ATTEMPT C:\CoworkV2\state\attempts\ORIGINAL_ATTEMPT\mount\capsule-ORIGINAL_UUID C:\CoworkV2\orphan-plan.json
cowork-process-runner.exe orphan confirm C:\CoworkV2\state\runner.json C:\CoworkV2\orphan-plan.json EXACT_PLAN_SHA256 C:\CoworkV2\state\control-key.pk8
cowork-process-runner.exe orphan receipt C:\CoworkV2\state\runner.json C:\CoworkV2\orphan-plan.json EXACT_PLAN_SHA256
```

`inspect` verändert kein OS-Profil. Sein vom ursprünglichen Runner signierter
Plan bindet Originaljournalbytes, native 128-Bit-Dateiidentitäten, Profil-SID,
Betreiberkonto, ursprüngliche VHD/Volume-Zuordnung und den vollständigen kleinen
Profilbaum einschließlich Dateihashes. Der Betreiber prüft den Plan; `confirm`
mit genau dessen SHA und dem ursprünglichen Kontrollschlüssel bestätigt die
Zuordnung dieses Profils zum ursprünglichen Erstellungsversuch. Der neue
Prüfplan gilt 15 Minuten. Aktive Ownerlocks/Jobs, fremde VHDs, ausgetauschte
Dateien, Reparse-/Hardlinks, bereits gestartete Capsulephasen oder eine fremde
Signatur werden abgewiesen. Begrenzungen: 256 Profileinträge, Tiefe 8,
32 MiB gesamte Profildateien und 64 KiB kanonischer Profilbeleg.

Die signierte Absicht liegt vor OS-IO dauerhaft außerhalb der VHD. Die
Retirementquittung verleiht keine Ausführungsrechte und legt keinen `owned`-
Beleg an. Nach Antwortverlust liefert derselbe Auftrag die Originalquittung;
fehlt diese noch, darf nur die native Abwesenheit der ursprünglichen
Registrierung und des ursprünglichen Ordners den Abgleich abschließen. Ein
wieder vorhandenes Profil wird dabei niemals erneut gelöscht. Der Receiver
verweigert Diskdetach, bis eine passende unabhängige signierte Quittung
vorliegt. Quittungen bleiben auch nach späterem Detach lesbar.

Alle Produktmutationen dieses Profils verwenden zusätzlich eine per-user
Lifecycle-Sperre; der kontrollierende Betreiberaccount hält das OS-Profil während
der Klärung in seinem Wartungsfenster. Windows verlangt vor
[`DeleteAppContainerProfile`](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-deleteappcontainerprofile)
geschlossene Profil-Dateihandles. Die Klärung ist keine Garantie gegen
gleichzeitige externe Manipulation durch diesen privilegierten Account.
Native Profileanlage/-entfernung, Originalbeleg-Replay und abgewiesene
Neuerstellung unter altem Namen bestehen in einer engen eigenen Windowsfixture.
Der komplette reale VHD-/Dienst-/CLIablauf und Powerloss bleiben separat
abzunehmen. Historische Runnerarchive enthalten diese Ergänzung nicht.

## Linux: echte gVisor-Container

Der explizit ausgewählte Dockerdaemon muss den `runsc`-Runtime tatsächlich
registriert haben. Er braucht quota-fähigen `overlay2`-Speicher, üblicherweise XFS
mit aktivierten Projektquoten. Ohne tatsächliche Unterstützung von `--storage-opt
size` wird kein Ersatzcontainer gestartet. Der Benutzer muss den ausgewählten
Dockerdaemon bedienen dürfen; nur sein absoluter Dockerpfad wird verwendet.

Beim Konfigurieren ergänzen:

```text
--docker /usr/bin/docker --linux-image registry.example/runtime@sha256:EXAKTER_64_HEX_DIGEST
```

`--linux-image` ist wiederholbar. Der erzeugte `install-images.py` führt erst bei
einem späteren ausdrücklichen Aufruf den Pull des gepinnten Images und eine
RepoDigest-Prüfung aus. Private Registryanmeldung bleibt beim Betreiber-Docker,
außerhalb von Ablaufdefinitionen. Bereits lokal installierte Images können
direkt verwendet werden. Der Receiver selbst zieht beim Start kein anderes Image.

Die UI-Version verwendet diesen Digest und paketrelative Programmpfade, etwa
`python=usr/bin/python3`. Jedes Containerexemplar verwendet tatsächliches `runsc`,
kein Netzwerk, keine Hostmounts, keine Linux-Capabilities, no-new-privileges sowie
exakte CPU-, Memory-, PID- und Scratchlimits. Die vertrauenswürdige native Probe
muss mit dem Image kompatibel sein; erforderliche Laufzeitbibliotheken gehören
zum gepinnten Image. Die Qualifizierung deckt inkompatible Images auf.

Der erzeugte `install-service.py` installiert erst bei ausdrücklichem Aufruf eine
neue systemd-Benutzereinheit, aktiviert sie und startet sie. Für Betrieb bereits
vor einer Anmeldung muss ein Administrator Linger für den dedizierten Betreiber
ausdrücklich einrichten. `uninstall-service.py` entfernt nur diese genaue Einheit;
private Daten bleiben erhalten. Die Einheit startet nach Abbruch neu; der
Receiver beendet dann exakt eigene Container, bevor neue Arbeit angenommen wird.

## Physische Qualifizierung und Aktivierung

„Installieren und physisch qualifizieren“ in der Umgebungsliste verbindet sich
mit dem privaten Receiver und startet die native Probe in genau der definierten
Isolation. Diese muss Hostlesen, Hostschreiben und Netzwerk tatsächlich abweisen,
Credentialisolation zeigen, im Workspace schreiben können und ein Kind starten.
Der Adapter bestätigt danach das Ende aller eigenen Prozesse. Ein signiertes
Ergebnis allein ersetzt diese physischen Prüfungen nicht.

Danach „Erneut prüfen und aktivieren“ wählen. Der Daemon übernimmt nur die exakte
qualifizierte, aktive Versionsreferenz. Programme/Abläufe/Zeitpläne können diese
Referenz über den Umgebungspicker verwenden. Widerruf oder geänderte Bindung
verhindert neue Starts. Ein Binaryupdate erfordert eine neue explizite Runner-
Bindung; vorhandene Signaturjournale werden nicht umgeschrieben oder gelöscht.

## Workspace, Abbruch und Retention

Snapshots erhalten auch leere Ordner. Jede Promotion prüft den vollständigen
ursprünglichen Zustand unter derselben FileBroker-OS-Sperre, protokolliert den
vollständigen Vorher-/Nachherzustand vor der ersten Änderung und flusht Dateien
und Verzeichnismetadaten vor dem Erfolgsbeleg. Eine teilweise Promotion bleibt
ungeklärt; der Receiver führt sie nach einem Crash nicht nochmals aus.

Ein HTTP-Abbruch beendet keine eigenständige Supervisoraufgabe. Die Supervisor-
schleife prüft Abbruch, Fence/Lease und Deadline. Ein unbestätigtes Prozessende
stoppt weitere Admissions und führt in eigene Recovery. Ein verlorener Reply
kann nur durch den originalen signierten Receipt geklärt werden, nicht durch
erneutes Senden einer Aktion.

Abgeschlossene Container werden ohne `--force` nach tatsächlichem Stop entfernt.
Windows prüft die exakte VHD-/Mountidentität und den nativen `IS_LOADED`-Zustand,
bevor eine eigene VHD entfernt wird. Snapshots und Attemptjournale werden nach
der Retention gelöscht; signierte Brokerreceipts bleiben in der privaten SQLite-
Datenbank. Ein separates Garbagejournal macht eine unterbrochene Bereinigung
fortsetzbar. Jede rekursive Löschung prüft den exakten eigenen Zielpfad und den
gesamten Baum auf Links/Reparse Points; fremde oder ungeklärte Ressourcen bleiben.

Standard: sieben Tage Retention und maximal 256 gleichzeitig zurückgehaltene
Versuche. `--retention-seconds` erlaubt eine Stunde bis 90 Tage,
`--max-retained-attempts` 8 bis 10.000. Das Limit stoppt neue Admissions; es löscht
keine ungeklärten Belege. Die Bereinigung läuft beim Start und während des Betriebs
alle 60 Sekunden, wenn kein eigener Job aktiv ist. Bei Uhr-Rücklauf wird keine
vorzeitige Löschung bestätigt. Installation, echter OSbetrieb, Wiederanlauf und
Crash-/Quotaabnahme müssen auf dem konkreten Zielgerät erfolgen.
