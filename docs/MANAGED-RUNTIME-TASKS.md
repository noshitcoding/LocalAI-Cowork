# Eigener gVisor-Taskpfad: Entwicklungsstand

Dieser Baustein implementiert tatsächliche containerd-/gVisor-Aufträge. Er ist
noch kein ausgelieferter Runtime-Dienst, kein freigegebenes Plattformprofil und
kein Abschluss von Paket C oder des A–I-/M0–M10-Auftrags. Ein aktueller, an seine
Quellen gebundener Nachweis wird separat beigefügt; ältere Berichte qualifizieren
die hier weiterentwickelten Quellen nicht automatisch.

**Aktuelle Abnahme offen (30.10.2026):** Die neuen Quellen trennen echte
Gast-Waitbelege vom internen Shimstatus, übernehmen verzögerte Fence-Children in
einen Reaper und verlangen nicht austauschbare Paketpfade. Der gemeinsame
portable Eingabevertrag liegt jetzt in `cowork-runner-protocol`. Ein
reproduzierter Konkurrenzfehler der Broker-Zeitmessung ist korrigiert: echte
Zeit wird innerhalb der serialisierten Zulassungsentscheidung gemessen, ohne
den Clock-High-Water-Mark zu begrenzen oder zurückzusetzen. Die zugehörigen
Windows-/Linux-Protokollprüfungen bestehen mit jeweils 25 Fällen.

Die aktuelle vollständige physische Probe besteht noch nicht. Ein unabhängiger
Monitor erfasste bei 132453 serialisierten WSL-Zeitmessungen einen tatsächlichen
UTC-Rücksprung von 784439449 ns. Der zugehörige Auftrag wurde beim Vergleich
`high_water=1793371299862`, `observed=1793371299140` gestoppt. Diese Beobachtung
wird nicht als erfolgreicher Deadline-Test umgedeutet. Mounts, Loop-Geräte und
die eigene Fixture-Generation der Probe wurden positiv stillgelegt. Details
stehen im Clock-/Abnahmestand (development artifact omitted from public sources).

## Eintritt und Auftragsgrenze

`ManagedTask` verlangt den nicht kopierbaren `DispatchedLaunch` des bestehenden
gemeinsamen Brokers, `VerifiedTaskInput`, ein signiertes qualifikationsgebundenes
`ManagedProfile` und vorreservierte native Ressourcen. Er erzeugt keine Pläne,
Projektverwaltung oder zweite geschäftliche Laufdatenbank. Das private physische
Journal beschreibt ausschließlich die Eigentums-, RPC- und Abschlusszustände
dieses konkreten Auftrags.

Eingabesnapshot, Zielrevision, tatsächliche Runner-Identität, Generation,
Umgebung, Invocation, Worker-Fence, Lease-Epoche und Anfragehash müssen exakt zum
bestätigten Eingabegrant passen. Der vorhandene Empfänger verifiziert und
materialisiert alle Manifestbytes, bevor der Broker einen Dispatch übergibt.
Nur dessen native Arbeitskopie und ein eigenes temporäres Verzeichnis werden
eingebunden. Hostoriginale, Runtime-Sockets und Zugangsdaten werden nicht gemountet.
Dateiidentitäten bleiben lokale Merkmale und werden nicht zwischen Rechnern
übertragen.

## Speicher, Prozesse und Netz

Der Arbeitsbereich verwendet ein vorab physisch reserviertes ext4-Volume mit den
Bytes und Inodes des Umgebungsprofils. Ein separates reserviertes Steuer-Volume
mit 16 MiB und 4096 Inodes enthält containerd-Zustand, Runtime-Metadaten, begrenzte
Logs, FIFOs und Abschlussbelege. Ein voller Workspace kann diese Steuerdateien
nicht verdrängen. Zwei vorreservierte Journalslots und ein vorreservierter
Ergebnisslot erlauben den Abschluss ohne nachträgliche Speicherzuteilung.

Die eigene containerd-Generation läuft pro Auftrag in einer privaten
Mount-Namespace. Ein fester interner Pfad hält Unix-Socketadressen kurz. Die
Abbildung verwendet nach dem Namespace-Wechsel erneut geprüfte lokale native
Verzeichnisse und `open_tree`/`move_mount`; Pfadgleichheit genügt nicht. Der
App-Prozess erhält dadurch keine neuen Mounts. Bootstrap-Temporärdaten liegen
zusätzlich in einer privaten, auf 8 MiB und 128 Inodes begrenzten `/run`-Umgebung.

Der neue Executor-Eintritt platziert ausschließlich den neu geforkten
Kontrollprozess vor `exec` im eigenen cgroup-Zweig. Der Runner-Prozess wird nicht
verschoben. Die gemeinsame cgroup begrenzt CPU, RAM, Swap und Hostprozesse von
containerd, Shims und gVisor. Der geschützte Workload-Vorfahre und der
Steuerzweig sind getrennt; gVisor erhält nur seinen untergeordneten Blattpfad.
Eine verspätete Erstellung kann den eingefrorenen Vorfahren nicht entsperren.

Rootfs und vollständiger gVisor-Werkzeugsatz werden gegen gebundene Manifeste und
native Merkmale geprüft. Die OCI-Spezifikation ist fest erzeugt: UID/GID 1000,
keine Capabilities, `noNewPrivileges`, schreibgeschütztes Rootfs, eingeschränkte
Mounts, feste Dateideskriptorlimits und Netzwerkmodus `none`. Ein PATH-Fallback
auf Hostprogramme ist nicht vorhanden. Proxyprofile werden durch diesen
Entwicklungsadapter noch nicht unterstützt.

## Workload-Status und physischer Abschluss

Pro Auftrag existieren eine eigene gehaltene Sandbox mit einem versiegelten
Pause-Programm und ein davon getrennter Workload. Die Sandbox hält den Sentry
verfügbar, während der Workload seinen tatsächlichen Exitstatus liefert. Dies
vermeidet die beobachtete Statuslücke sehr kurzlebiger Sandbox-Rootprogramme.
Die beiden IDs dürfen sich nicht gegenseitig präfigieren, weil gVisor verkürzte
Container-IDs akzeptiert. Neue physische Journale verwenden wegen der
zusätzlichen Pflichtbelege Format 2; alte Format-1-Bytes bleiben historisch und
erzeugen keine neue Ausführungsautorität.

Die unabhängige Prüfung dieses ersten Acht-Job-Stands fand eine zusätzliche
Statuslücke: Auch bei laufender Pause-Sandbox kann der Shim einen internen
`runsc wait`-Fehler als 128 zwischenspeichern. Der historische Acht-Job-Beleg
prüft diesen Fehlerfall nicht. Die laufende Korrektur verlangt deshalb einen
zusätzlichen gepinnten Runtime-Treiber. Er hält den vorab geprüften echten
runsc-ELF-Deskriptor, beobachtet dessen ursprünglichen Child, vollständiges EOF
und die erfolgreiche JSON-Waitantwort und speichert sie vor Rückgabe an den
Shim in einem vorreservierten geschützten Slot. Frische Nonce, Anfragehash,
Container-ID und Runtime-Bindungen verhindern die Verwendung fremder oder alter
Belege. Fehlende, beschädigte oder fehlgeschlagene Waitbelege bleiben ungeklärt.
Ein echter Gast-Exitcode 128 bleibt davon unterscheidbar. Diese neuen Bytes und
der echte Readonly-Dateisystemfehler benötigen ihren eigenen aktuellen Beleg.

Der Adapter unterscheidet den tatsächlichen Workload-Exit, den rohen
Sandbox-/Shim-Status und das Ende des ursprünglichen Kontrollprozesses. Ein
verlorener Gaststatus wird nicht allein durch eine leere cgroup oder ein
erfolgreiches Delete ersetzt. Bei erzwungenem Abbruch bleibt ein nicht gemessener
Gast-Exitcode leer; der rohe Runtime-Status bleibt im physischen Journal.

Jede mutierende RPC wird zuerst dauerhaft als ausstehend gespeichert. Quittungen
enthalten die betroffene Container-ID und unterscheiden Erfolg von bekannten
negativen Kill-Antworten. Der erwartete Ablauf für beide Container ist fest
geordnet. Unbekannte Erstellungs- oder Startwirkungen führen zu keinem weiteren
Dispatch.

Ausgabe wird im Speicher begrenzt und bei Überschreitung mit Rückstau versehen.
Nach belegter Leere des geschützten Workload-Zweigs dürfen noch vorhandene
Pipepuffer abfließen, damit der Shim seine Kopierprozesse beenden kann. Dies ist
noch kein EOF- oder Abschlussbeleg. Erst beide bestätigten Task-Deletions,
Metadaten-Deletions, native Ressourcenprüfung, Reaping des ursprünglichen
Kontrollprozesses und tatsächlich beobachtetes FIFO-EOF erlauben einen
vollständigen Abschluss. Die tatsächlichen Ergebnisbytes werden vor dem finalen
Journalbeleg dauerhaft gespeichert.

Veröffentlichte Arbeits- und Steuer-Volumes bleiben auch danach gebucht. Eine
Produkt-API für deren Freigabe nach Ergebnisübernahme ist noch nicht vorhanden.

## Unabhängiger Wächter

Ein verpflichtender eigener Prozess erhält nur den originalen Owner-pidfd, den
exakten nativen Workload-Verzeichnisdeskriptor, eine feste BOOTTIME-Frist und ein
privates geerbtes Socketpaar. Er hat keinen Listener, keine allgemeinen
Administrationsbefehle und übernimmt keine fremden Prozesse anhand einer PID.
Owner-Tod, Kanalverlust und Fristablauf frieren den eigenen Vorfahren ein und
fordern das Ende seiner aktuellen Population an. Die Sperre bleibt bestehen.

Die physische Frist überschreitet weder Invocation-Timeout, Ausführungsdeadline
noch die anfängliche signierte Lease. Der Wächter kann verlängerte Laufrechte
noch nicht prüfen. Lange Aufträge mit Lease-Erneuerung sind damit noch kein
unterstützter Produktpfad. Eine Threadüberwachung allein wird nicht als Ersatz
verwendet. Abgeschlossene Ersteller, bestätigte Löschungen und entfernte eigene
native Ressourcen sind Voraussetzung für das ausdrückliche Entwaffnen.

Bei verzögertem Fence-Ende übernimmt ein bereits vor dessen Start eingerichteter
prozessweiter Reaper den ursprünglichen Child-Handle nach dem begrenzten
Drop-Wartefenster. Er sendet keine Signale und gibt keine Ressourcen frei. Die
separate physische Prüfung dieses verzögerten Endes ist noch ausstehend.

## Prüfung und verbleibende Arbeit

```powershell
python tools/managed_runtime/probe_tasks.py --wsl-distro Ubuntu-24.04
```

Die Prüfung verwendet echte, exakt gepinnte containerd-/gVisor-Binaries, native
ext4-Volumes und tatsächliche Gastprogramme. Die Schlüssel, Qualifikation und
Planmetadaten sind ausdrücklich Fixture-Metadaten. Der gebundene Prüfabschluss
umfasst sechs reguläre Abläufe sowie Owner-SIGKILL und unabhängigen Lease-Ablauf
bei per SIGSTOP angehaltenem Owner. Die gesamte eigene Fixture-Generation wird
getrennt stillgelegt; diese Stilllegung ist kein Produkt-Freigabebeleg für
ungeklärte Läufe.

Der Snapshot-Nachweis (development artifact omitted from public sources) bindet 178 stabile
Quelleninputs und acht tatsächlich gestartete Gastaufträge. Sechs Abläufe liefern
getrennte Task-/Sandbox-/Kontrollprozess-/Ausgabebelege; zwei absichtlich beendete
Owner bleiben ungeklärt und gebucht. Clippy des eigenen Featurepakets und die
Formatkontrolle bestehen. Alle eigenen Mounts, 17 Loop-Verbindungen, cgroups und
Werkzeugkopien sind stillgelegt bzw. entfernt. Das geprüfte, unsignierte
Entwicklungsartefakt (private development evidence; omitted from public sources)
hat SHA256 `2fdc4d6a6fce9fa8982b48caf8c843fd7638cfba6cb70f40fa17870fac5b3b4b`.
Die enthaltenen Quellbytes sind zusätzlich im geprüften Quellenarchiv mit SHA256
`19cb7c7c4eae50f3c8ba68baa90ceb7f8332bc9ae31c2d26eb9c7258babc0385` gebunden.
Weiterentwickelte Quellen benötigen einen neuen Nachweis.

Offen bleiben insbesondere die sichere physische Lease-Erneuerung,
Wiederherstellung teilweise erfasster Konsolenausgaben nach hartem Owner-Tod,
Ergebnisübernahme und Volume-Freigabe, produktive Generationsstilllegung,
App-Grant-Minting und Transferendpunkte, Server-/Geräteanbindung, globale
Ressourcenzulassung, vollständige Plattform-/Bootfingerprint-Zulassung,
signierte Installation/Updates/Rollback, eigene ausgelieferte
WSL-Distribution und native Linux-Hardwareabnahme. CPU-/RAM-/Forkbomb-Stress und
die vollständige Plattform- und M10-Matrix sind durch diese Fälle nicht ersetzt.
Team-Readiness und Produktfreigaben bleiben geschlossen.

Ein kalter Sandbox-Start benötigt im geprüften Profil mit einer CPU mehrere
Sekunden. Kurze Auftragsfristen können bereits während des Bootstrap ablaufen;
dieser Fehler wird nicht durch spätere Startrechte oder ein schwächeres Profil
überbrückt. Der aktive Deadline-Fall verwendet ausdrücklich 23 Sekunden und
verlangt eine vom Gast geschriebene Startmarkierung. Eine allgemeine Zusage für
kürzere Starts oder die M10-Latenzen ist damit nicht verbunden.

Die Schnittstellendetails wurden gegen die gebundenen Primärquellen geprüft:
[gVisor-Taskadapter](https://raw.githubusercontent.com/google/gvisor/release-20261026.0/pkg/shim/v1/runsc/service.go),
[gVisor-Runtimeoptionen](https://raw.githubusercontent.com/google/gvisor/release-20261026.0/pkg/shim/v1/runtimeoptions/runtimeoptions.proto),
[containerd-Taskservice](https://raw.githubusercontent.com/containerd/containerd/v2.2.1/plugins/services/tasks/local.go)
und [containerd-Typregistrierung](https://raw.githubusercontent.com/containerd/typeurl/v2.2.3/types.go).
