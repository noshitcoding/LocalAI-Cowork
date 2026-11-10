# Privater containerd-Dienst und gemeinsame Dispatchübergabe

`crates/managed-runtime/src/containerd.rs` und `containerd/daemon.rs` enthalten
jetzt die tatsächliche private Dienstverwaltung und die offiziellen
containerd-gRPC-Bindings. Das ist ein implementierter Runtime-Baustein. Die
gemeinsame Plan-/Snapshot-/Programmausführung und die ausgelieferte Runtime
bleiben offen; kein Ausführungsprofil wird damit freigegeben.

## Originalprozess, Dateideskriptor und Socket

Die vertrauenswürdige Einrichtung liefert den erwarteten SHA256 des ausführbaren
ELF aus ihrer Paket-/Operatorkonfiguration. Der Dienst akzeptiert ausschließlich
eine unveränderlich geöffnete, schreibgeschützte, Root gehörende reguläre Datei
ohne zusätzliche Hardlinks. Der offene Deskriptor bleibt bis zum ursprünglichen
Spawn erhalten. Ein anschließend ersetzter Quellpfad wird nicht ausgeführt.

Der vorhandene Prozess-Supervisor beobachtet ausschließlich das neu gestartete
Kind. Die Verbindung bindet dessen PID, Linux-Startzeit, UID und tatsächlich
ausgeführten ELF-Hash. Der Unix-Socket bindet seine lokale Device-/Inode-ID,
Eigentümer und private Elternverzeichnis-ID. Jeder Verbindungsaufbau prüft
zusätzlich `SO_PEERCRED`; auch eine Wiederverbindung darf keinen anderen
gleichnamigen Dienst übernehmen. Vor und nach der Versionsabfrage werden die
Bindungen erneut geprüft. Ein bereits offener HTTP/2-Kanal versteckt deshalb
keinen ausgetauschten oder inzwischen zu weit freigegebenen Socket.

Diese nativen Angaben bleiben lokale Kontrollmerkmale. Der aus der geprüften
Runnerbindung abgeleitete containerd-Namespace ist keine Zugriffsberechtigung.
Der Transport veröffentlicht weder einen TCP-Port noch eine öffentliche Start-API.

## Privater Kontroll-Namensraum und Lebenszyklus

containerd 2.2.1 legt trotz einer eigenen `--state`-Angabe zusätzlich seinen
fest eingebauten Standardpfad `/run/containerd` an. Das steht auch im
[offiziellen Quelltext](https://github.com/containerd/containerd/blob/v2.2.1/cmd/containerd/server/server.go#L87).
Deshalb erzeugt nur der neue Supervisor-Thread einen eigenen Mount-Namensraum
mit privater Propagation und einem begrenzten `/run`-tmpfs: 8 MiB, 128 Inodes,
`nodev`, `nosuid`, `noexec`. Der ursprüngliche App-/Host-Namensraum bleibt erhalten.

Root, State, temporäre Dateien, Konfiguration und Sockets liegen in einem eigenen
Root gehörenden 0700-Verzeichnis. Fremde Konfigurationsimporte und die für diesen
Kontrollpfad unbenötigten CRI-, NRI-, Opt- und externen Image-Verifier-Plugins sind
deaktiviert. Der Kindprozess erhält nur die explizite Umgebung. Ausgabe wird je
Stream bis 64 KiB behalten und weiter abgeleitet. Die Dienstlebensdauer bleibt
begrenzt. Das Kontrollverzeichnis muss später aus separat reserviertem Speicher
kommen; die gemeinsame Kapazitätszulassung ist damit noch nicht integriert.

Ein dauerhaft geschriebener Start-Fence und ein nativer exklusiver Lock gehen dem
Spawn voraus. Ein nicht leeres oder zuvor verwendetes Verzeichnis wird nicht
erneut gestartet oder als vorhandener Dienst adoptiert. Abbruch der Einrichtung,
explizites Stoppen und Verlust des Besitzerobjekts stoppen ausschließlich das vom
Supervisor selbst gestartete Kind. Sie löschen keinen Fence und geben keine
Workspace-/Kontrollreservierung frei. Ein beendeter containerd-Prozess beweist
ausdrücklich nicht, dass ein Workload oder dessen Shim beendet ist. Wiederanlauf
und Generationsstilllegung brauchen die noch anzubindenden physischen Belege.

## Einmalige Übergabe aus dem vorhandenen Broker

`cowork_runner_protocol::Broker::dispatch_owned` verwendet dieselbe vorhandene
dauerhafte Dispatchtransaktion wie `dispatch`. Sie liefert einen nicht klonbaren,
nicht serialisierbaren `DispatchedLaunch`. Quittungen, kopierte Requestbytes und
ein Neustart können diesen Wert nicht erzeugen. Der bestehende ProcessRunner
überträgt ihn bereits an seinen unabhängigen Worker. Ein abgebrochener HTTP-
Aufruf besitzt diesen Worker nicht.

Geht die Übergabe vor einer beobachteten Wirkung verloren, bleibt der ursprüngliche
Datensatz `DispatchCommitted`. Wiederholung liefert dieselbe Quittung ohne neue
Übergabe. Es entsteht kein zweites fachliches Ausführungsjournal. Der Wert ersetzt
weder den bestätigten PlanV2 noch einen verifizierten Eingabesnapshot oder einen
physischen Abschlussbeleg. Der Legacy-Workspace-Identifier berechtigt insbesondere
zu keinem neuen Remote-Snapshot-Start.

## Gebundene Entwicklungsprüfung

Der aktuelle Nachweis (development artifact omitted from public sources) und sein
unsigniertes Review-Artefakt enthalten 39 stabile Kontroll-/Paketinputs und ein
separates stabiles Fenster mit 150 gemeinsamen Broker-/Service-Abhängigkeiten.
Drei tatsächliche WSL2-Fälle verwenden den exakten offiziellen containerd-2.2.1-ELF:
Prozess-/Socketverwechslung und Rechteänderung, eigener Dienstlebenszyklus samt
unveränderten Eltern-Mounts und Start-Fence sowie Dateipfadersatz nach ELF-Prüfung.
Die eigenen Dienste sind beendet, der äußere private `/run` ist normal ausgehängt
und die eigene Fixture ist entfernt. Es wurden keine Workload-Tasks gestartet.

Zusätzlich bestehen 21 Brokerfälle je Windows/WSL, elf Windows- und sechs Linux-
ProcessRunner-Fälle. Paketregressionen, Builder, Clippy einschließlich des
Linux-Features und Formatprüfung bestehen. Der ignorierte PostgreSQL-Fall bleibt
ausdrücklich außerhalb dieses Nachweises. Frühere fehlgeschlagene Prüfungen werden
nicht überschrieben.

```powershell
python tools/managed_runtime/qualify_containerd.py --wsl-distro Ubuntu-24.04
```

Das optionale Linux-Feature `containerd` nutzt `containerd-client = 0.9.0` aus den
[offiziellen Rust-Bindings](https://github.com/containerd/rust-extensions/tree/main/crates/client).
Der Build holt den fest gebundenen Protobuf-Compiler nur in das private
Projektverzeichnis. Das installiert weder einen Systemdienst noch eine Runtime.

Als Nächstes fehlen die Bindung an bestätigte Pläne und Empfängerrechte, vollständig
verifizierte Inputs auf den reservierten Volumes, gVisor-Taskstarts, geschützte
IO-Erfassung, CPU/RAM/PID-/Gesamtspeichergrenzen, Netzwerkvermittlung und genaue
Task-/Shim-Abschlussbelege. Erst diese Integration kann einen gemeinsamen lokalen
und Server-Produktpfad und anschließend eine Plattformfreigabe liefern.

Der anschließend separat geprüfte [private Eingabe-/Grantbaustein](MANAGED-RUNTIME-INPUTS.md)
verbindet inzwischen den vorhandenen Receiver, reservierte Volumes und den
gemeinsamen Dispatch. Die App-Ausstellung, Übertragungsendpunkte und tatsächlichen
Taskstarts bleiben weiterhin offen. Dieser neue Nachweis erweitert nicht den
oben gebundenen ursprünglichen Kontrollprozess-Nachweis.
