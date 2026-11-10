# Reservierte Volumes für die eigene Runtime

`crates/managed-runtime/src/volumes.rs` und `volumes/loop_device.rs` implementieren
die native Linux-/WSL-Speichergrenze für den kommenden Runtime-Dienst. Die
Implementierung verwaltet ausschließlich Ressourcen. Sie enthält keine
Projektverwaltung und erzeugt aus Requesthashes keine Ausführungsberechtigung.
Der gemeinsame Runner muss einen bestätigten Auftrag zuerst selbst zulassen.

## Tatsächliche Grenzen und lokale Identität

Ein Pool begrenzt gleichzeitig reservierte Nutzdatenbytes und aktive Volumes.
Jedes Volume erhält eine mit `posix_fallocate` tatsächlich belegte Image-Datei,
ein eigenes ext4-Dateisystem und eine feste Inode-Grenze. Ein sparse Fallback
existiert nicht. Die aktuelle Implementierung akzeptiert ausschließlich ext4
als Speicherunterbau; neben der gemeinsamen ext-Magic wird der tatsächliche
Kernel-Mounttyp geprüft. Nicht unterstützte Dateisysteme werden abgewiesen.

Ein begrenzter, an seinen tatsächlichen Hash und offenen Dateideskriptor
gebundener `mke2fs`-Aufruf erzeugt das Dateisystem. Das Loop-Gerät wird direkt
über `LOOP_CONFIGURE` an den bereits offenen Image-Deskriptor gebunden. Vor
Wiederverwendung und Abbau werden Image-Inode/Device, privater Nonce, Loop-
Identität, Limits und Mount-ID geprüft. Diese Angaben bleiben private lokale
Journalmerkmale; sie werden nicht als portable Ressourcenidentität ausgegeben.
Ein Pool allein authentisiert weder Rechner noch Runner. Diese Bindung bleibt
Aufgabe der tatsächlichen gemeinsamen Runtime-Registrierung und Zulassung.

Mounts erzwingen `nodev`, `nosuid` und `noexec`. Die Dateisystemwurzel gehört
Root und bleibt privat. Ausschließlich das eigene Unterverzeichnis `work`
gehört UID/GID 1000 und kommt als Daten-Mount für einen Workload infrage.
`data_directory()` prüft dessen native Identität. Das Kontrollverzeichnis und
die Namespace-Journale dürfen nicht an Workloads übergeben werden. Übertragene
Dateien benötigen vor dem späteren Programmstart ihre passenden Datenrechte.
Diese Berechtigungen ersetzen gVisor und die übrige Isolation nicht.

## Journale, Abbruch und Wiederaufnahme

Zwei vorab reservierte 8-KiB-Datensätze speichern Zustand, Sequenz und Prüfsumme.
Zustandswechsel ersetzen keine Dateien und benötigen keine neuen Datenblöcke.
Dadurch funktioniert der Abgleich auch dann, wenn eine fehlgeschlagene
Reservierung das zugrunde liegende Dateisystem gefüllt hat. Ein beschädigter
Datensatz erzeugt keine neue Materialisierung oder Formatierung. Vor einem
Dispatch wird zusätzlich ein eigener dauerhafter Publish-Fence angelegt:
ein gerissener neuester Datensatz kann einen veröffentlichten Workspace nicht
wieder zu einer ungestarteten Ressource machen.

Eine unveröffentlichte Reservierung lässt sich ausdrücklich abgleichen und
verwerfen. Fehlendes oder ersetztes Material wird nicht neu erzeugt. Ein
belegtes Mount oder eine noch aktive Loop-Referenz verhindert die Freigabe.
Insbesondere ist ein erfolgreiches `LOOP_CLR_FD` allein kein Beweis für eine
Trennung: Die Implementierung verlangt den tatsächlichen ungebundenen Zustand.
Erst nach bestätigtem Aushängen, getrenntem Gerät, Löschung des exakt eigenen
Images und dauerhaftem Abschluss wird Nutzdatenkapazität wieder frei.

Veröffentlichte Volumes bleiben derzeit gebucht. Es gibt bewusst noch keine
Produkt-API, die sie ohne gemeinsamen physischen Stopbeleg freigibt. Der nächste
Adapter muss Container-/Prozesszustand und unbekannte Dispatch-Wirkungen über
das vorhandene ProcessRunner-Journal abgleichen. Ein leeres Cgroup oder ein
verlorener Netzwerkaufruf allein genügt nicht.

Der Nutzdatenpool enthält höchstens 128 aktive Volumes und 4.096 behaltene
Ressourcendatensätze. Das Byte-Limit bezeichnet die Nutzdatenreservierungen.
Begrenzter Kontrollspeicher, Logs, containerd-Cache und deren gemeinsame
Rechenzulassung sind eigene noch anzubindende Reservierungen. Diese Teil-
Implementierung ist deshalb keine vollständige Runtime-Kapazitätsfreigabe.

## Physischer Entwicklungsnachweis

Der gebundene Nachweis (development artifact omitted from public sources) enthält
13 tatsächliche WSL-/Linux-Fälle: volle Daten- und Inode-Dateisysteme,
konkurrierende Pools, Wiederöffnung mit bearbeiteten Dateien, erschöpften
Unterbau, native Ersetzung, besetzte Mounts, verbleibende Loop-Referenzen,
gerissene Journale vor/nach Veröffentlichung, UID-1000-Datenrechte und
Abweisung eines tmpfs-Unterbaus. Die eigene 512-MiB-Außenfixture mit ihren
verschachtelten Mounts, Loop-Geräten und Testbytes ist positiv entfernt.

Zusätzlich bestehen sieben Windows- und acht normale Linux-Fälle des
Paket-/UAPI-Kerns, jeweils vier Builderfälle, Clippy und Formatprüfung. Das
Review-Artefakt bindet 32 stabile Quelleninputs und die tatsächlichen unsigned
Prüfprogramme. Kein fremder Prozess und keine fremde Distribution wurde beendet.

```powershell
python tools/managed_runtime/qualify_volumes.py --wsl-distro Ubuntu-24.04
```

Paket C bleibt offen: gemeinsamer Prozess-/Snapshotadapter, privater Dienst,
geschützte IO-Erfassung, CPU/RAM/PID-Grenzen, Netzwerkvermittlung, eigene WSL-
Paketierung, Install-/Update-/Rollbackpfad und tatsächliche Plattformabnahme
gehören weiterhin zur vollständigen Lieferung. Dieser Nachweis gibt weder ein
Runtime-Profil frei noch ersetzt er native Linux-Hardware- oder M10-Abnahme.

Die verwendeten Kernel-Schnittstellen beschreibt die
[offizielle Linux-UAPI](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/loop.h).
Die geplante containerd-Anbindung kann die
[offiziellen Rust-Bindings](https://github.com/containerd/rust-extensions/tree/main/crates/client)
verwenden; ein zweites fachliches Ausführungsjournal entsteht dadurch nicht.
