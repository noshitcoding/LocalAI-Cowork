# Eigene Runtime: Paketvertrag und Prüfwerkzeug

`crates/managed-runtime` stellt die gemeinsame native Paketprüfung für die
kommende WSL2-/Linux-Runtime bereit. `cowork-runtime-package` liest standardmäßig
nur; mit der ausdrücklichen Option `--stage` erstellt es zusätzlich eine
[geschützte Paketkopie](MANAGED-RUNTIME-STAGING.md). Es extrahiert und startet keine
Paketdatei, importiert keine Distribution und verändert keine Installation.
Eine bestandene Paketprüfung erzeugt keine Ausführungsautorität.

## Signierte Bytes

Ein Paket besitzt genau `manifest.json`, `manifest.sig.json` und `payload/`.
Das Manifest Format 1 bindet Plattform, Version, positive Release-Sequenz und
neun geordnete Bestandteile: Basis-Rootfs, Browser, containerd, Dokumentwerkzeuge,
Git, vollständiges gVisor-Paket, Node, Python und Runtime-Dienst. Jeder Bestandteil
bindet einen portablen einfachen Dateinamen, Version, Größe und SHA-256.
Modelle und lizenzierte Microsoft-Programme sind kein Bestandteil dieses Pakets.

Die Ed25519-Signatur gilt für die Bytes
`localai-cowork/runtime-package/v1`, ein einzelnes NUL-Byte und anschließend die
**exakten** Manifestbytes. Der Prüfer formatiert JSON vor der Signaturprüfung
nicht neu. Der öffentliche Vertrauensschlüssel kommt aus einer unabhängig
konfigurierten Quelle. Die CLI lehnt einen Schlüssel innerhalb des Pakets ab.
Ein Paket kann sich keinen eigenen Vertrauensanker mitliefern.

Nach der Signaturprüfung folgen Plattform- und Strukturprüfung sowie vollständige
streamende Prüfsummenprüfung. Zusätzliche Dateien, fehlende Rollen, falsche Größen,
doppelte Namen, Windows-Gerätenamen, Pfadkomponenten, Symlinks und Hardlinks werden
abgewiesen. FIFO-Ersatz wird unter Linux ohne blockierendes Lesen abgewiesen.
Metadaten sind auf 64 KiB begrenzt, einzelne Bestandteile auf 12 GiB und das
gesamte Paket auf 32 GiB; diese Eingangsgrenzen reservieren noch keinen Speicher.

Der Bericht unterscheidet `signature_verified` und `payload_verified` von
`installed`, `qualified` und `execution_authority`. Die letzten drei Werte
bleiben bei diesem Prüfwerkzeug immer `false`. Eine Release-Sequenz allein
erlaubt weder Aktivierung noch Rollback.

```powershell
cargo build --locked -p cowork-managed-runtime --bin cowork-runtime-package
target/debug/cowork-runtime-package.exe C:/ABS/runtime-package C:/ABS/trust/runtime.pub release-key-id windows_wsl2_x86_64
```

Der öffentliche Schlüssel enthält exakt 32 rohe Ed25519-Bytes. Native
Linux-Pakete verwenden `linux_x86_64`. Plattformen werden nicht gegeneinander
ausgetauscht, auch wenn einzelne Linux-Bestandteile dieselben Bytes besitzen.

Der ursprüngliche Nachweis am Ende dieses Dokuments bleibt ausschließlich an
seine damaligen Quellen gebunden. Der zusätzliche Stagingpfad besitzt einen
eigenen aktuellen Nachweis (development artifact omitted from public sources).

## Paketproduktion ohne private Schlüssel im Builder

`tools/managed_runtime/package.py prepare INDEX OUTPUT` kopiert ausschließlich
die im Index ausdrücklich ausgewählten, größen- und hashgebundenen Buildartefakte
in ein neues Verzeichnis. Der Index verwendet dieselben Manifestfelder; jeder
Bestandteil ergänzt das Feld `source` mit einem absoluten regulären Quellpfad.
Quellpfade werden nicht in das öffentliche Manifest übernommen. Der Builder
lädt keine Abhängigkeiten aus dem Netz und liest keinen privaten Signierschlüssel.

Die getrennte Release-Signierstelle signiert die oben beschriebenen Bytes und
liefert eine rohe 64-Byte-Signatur. Danach:

```powershell
python tools/managed_runtime/package.py seal C:/ABS/runtime-package C:/ABS/release.sig C:/ABS/trust/runtime.pub C:/ABS/cowork-runtime-package.exe release-key-id
```

`seal` ruft die native gemeinsame Paketprüfung auf. Eine falsche Signatur oder
beschädigte Datei erzeugt keinen erfolgreichen Bericht. Unvollständige bzw.
fehlgeschlagene Buildverzeichnisse bleiben unqualifiziert; Originaldateien
werden nicht gelöscht oder ersetzt. Signierung, Prüfung und Veröffentlichung
sind getrennte Operationen.

## Weitere erforderliche Integration

Diese Implementierung schließt Paket C nicht ab. Noch erforderlich sind der
eigentliche begrenzte Runtime-Dienst, private Kommunikation mit dem App-Daemon,
Anbindung der [reservierten Dateisysteme](MANAGED-RUNTIME-VOLUMES.md), kontrollierte Netzausleitung, gemeinsamer Dispatch mit
bestätigten Snapshot-/Modell-/Rechtebindungen und ein versionsgebundener Installer.
Letzterer muss auf geschützten Stagingbytes verifizieren, atomar veröffentlichen,
Anti-Rollback-Zustand behalten, die vorherige Version erhalten und neue Starts
bis zur echten Qualifikation sperren. Der Prüfer extrahiert keine Archive;
die spätere sichere Entpackung benötigt ihre eigene Begrenzung und Prüfung.

Die WSL-Einrichtung verändert nur die app-eigene Distribution. Die genaue
Trennung zwischen distributionsbezogener und globaler Konfiguration dokumentiert
[Microsoft](https://learn.microsoft.com/en-us/windows/wsl/wsl-config).
Der kommende Dienst soll den [containerd-gVisor-Adapter](https://gvisor.dev/docs/user_guide/containerd/quick_start/) verwenden.
Die vorliegende Paketprüfung bestätigt dessen physische Isolation nicht.

## Gebundener Entwicklungsnachweis

Der Paketprüfungsnachweis (development artifact omitted from public sources) bindet
Quellen, Prüfprotokolle und die tatsächlich gebauten unsigned Windows-/Linux-
Prüfprogramme. Sieben Windowsfälle (einschließlich echter Junction), sieben
WSL-/Linux-Fälle (einschließlich Symlink/FIFO), jeweils vier Builder-/Native-CLI-
Fälle, Clippy und Formatprüfung bestehen. Ein unabhängig in Python erzeugter
Ed25519-Testvektor prüft die gleichen öffentlichen Signaturbytes. Die Testschlüssel
und winzigen Payloads sind öffentliche synthetische Fixtures und qualifizieren
keine echten Runtime-/Werkzeugpakete.

```powershell
python tools/managed_runtime/qualify.py --wsl-distro Ubuntu-24.04
```
