# Signierte Runtime-Pakete geschützt bereitstellen

Die 13 geprüften Dateien sind im aktuellen Hauptstand integriert. Der frische
Main-Nachweis besteht mit 72 unveränderten Paketquellen, vier archivierten
Prüfprogrammen und 501 erhaltenen Workflow-/UI-/Vertragsdateien. Der frühere
private Nachweis bleibt separat erhalten; Installation und Aktivierung sind offen.

`cowork-managed-runtime::staging` ergänzt Paket C um eine begrenzte Kopie aller
neun signierten Komponenten in einem neuen Verwaltungsverzeichnis. Die native
CLI bietet denselben Pfad ausdrücklich an:

```powershell
cowork-runtime-package.exe C:/ABS/package C:/ABS/trust/runtime.pub release-key-id windows_wsl2_x86_64 --stage C:/ABS/private-staging attempt-1 21474836480
```

Der Verwaltungsprozess muss das übergebene Elternverzeichnis vorher unabhängig
vom Paket einrichten und dessen Dateisystem begrenzen. Staging verändert keine
Elternrechte und übernimmt kein vorhandenes Versuchverzeichnis. Das Nutzdatenlimit
begrenzt die Summe der signierten Dateigrößen auf höchstens 32 GiB. Zusätzliche
Dateisystemmetadaten, Blockrundung und mehrere aufbewahrte Versuche benötigen eine
eigene Volumequote; der Aufruf setzt keine globale Datenträgerquote durch.

Unter Linux muss das Verwaltungsverzeichnis dem aktuellen Verwaltungsbenutzer
gehören und Modus 0700 besitzen. Unter Windows werden Eigentümer und die DACL
über das gehaltene Verzeichnishandle geprüft. Zugelassen sind ausschließlich
der aktuelle Verwaltungsbenutzer, SYSTEM, Administratoren und die bereits an
den geprüften Eigentümer gebundenen OWNER-RIGHTS-Einträge. Unbekannte ACE-Typen,
fremde SIDs und eine fehlende DACL führen zur Ablehnung. Die Verwaltungsinstanz
ist eine Vertrauensgrenze; dieser Mechanismus isoliert keine Programme, die
bereits dieselben Verwaltungsrechte besitzen.

Manifest und Signatur werden gegen den unabhängig gelieferten Ed25519-Schlüssel
geprüft. Alle elf Zieldateien werden neu erstellt und physisch vorreserviert,
bevor Payloadbytes kopiert werden. Nicht unterstützte Vorreservierung und voller
Speicher führen zur Ablehnung; ein Sparse-Fallback existiert nicht. Metadaten
bleiben auf 64 KiB beziehungsweise 2048 Byte begrenzt. Beim Lesen unbekannter
Verzeichnisse wird schon das erste unerwartete Mitglied abgewiesen, statt ein
unbegrenztes Inventar im Speicher aufzubauen.

Die Kopie bindet die ursprünglichen Verzeichnisse und Dateien an ihre nativen
Identitäten. Windows verwendet die volle 128-Bit-Datei-ID mit Volumeseriennummer;
Linux verwendet Device und Inode. Diese Identitäten verlassen den lokalen
Verwaltungsprozess nicht. Komponenten liegen in festen Slots `component-0.bin`
bis `component-8.bin`; ihre Zuordnung folgt der signierten Rollenreihenfolge und
ist unabhängig von frei gewählten Paketdateinamen.

Beim Kopieren und anschließendem Lesen werden Länge und SHA-256 vollständig
geprüft. Die zweite Prüfung liest die erneut geöffneten ursprünglichen Dateien.
Windows hält danach schreib- und löschsperrende Lesehandles. Linux prüft zusätzlich
Eigentümer und private Dateirechte. Hardlinks, Reparse-Points, Symlinks, FIFO-Ersatz
und ausgetauschte Verzeichnis- oder Dateinamen erzeugen keine neue Zulassung.
Ein späterer `verify()`-Aufruf prüft erneut die gehaltenen Originale und alle Bytes.

`StagingArea` wird genau einmal verbraucht. `StagedPackage` besitzt weder
`Clone` noch `Deserialize` oder einen öffentlichen Konstruktor aus Metadaten.
Die CLI meldet `staged: true`, aber weiterhin `installed: false`,
`qualified: false` und `execution_authority: false`. Nach ihrem Ende kann ein
Installer diese native Verwahrung nicht aus dem JSON-Bericht rekonstruieren.
Fehlversuche bleiben begrenzt und sichtbar. Originale, vorherige Versionen und
unbekannte Verzeichnisse werden nicht entfernt.

Die Erfolgsquittung wird ausschließlich aus dem tatsächlich gehaltenen,
unmittelbar erneut geprüften `StagedPackage` gebildet. Eine frühere unabhängige
Paketprüfung liefert dafür keine Felder. Wird die Quelle inzwischen durch ein
anderes gültig signiertes Paket ersetzt, beschreibt die Quittung ausschließlich
die tatsächlich bereitgestellten neuen Bytes und deren Manifest.

Der aktuelle Entwicklungsnachweis (development artifact omitted from public sources)
bindet die tatsächlich gebauten unsigned Prüfprogramme an ihre Quellen:
13 Windowsfälle, 15 Linuxfälle, je vier Builderfälle, beide Clippy-Prüfungen und
Formatprüfung. Ein zusätzlicher ausdrücklich gestarteter Linuxfall füllt ein
eigenes Dateisystem mit 8 MiB tatsächlich bis `ENOSPC`, bevor Payloadbytes
kopiert werden. Dieser Nachweis verwendet tmpfs in einem privaten Mount-Namespace;
er belegt keine persistente Blockgeräte- oder native Linux-Hardwarequalifikation.
Der Mount wird regulär ausgehängt und der eigene leere Fixturepfad entfernt.
Öffentliche Testschlüssel und winzige synthetische Pakete qualifizieren keine
realen Runtime- oder Werkzeugpakete.

```powershell
python tools/managed_runtime/qualify.py --wsl-distro Ubuntu-24.04 --staging-enospc
```

Die ergänzte [begrenzte USTAR-Vorprüfung und Linux-Extraktion](MANAGED-RUNTIME-ARCHIVES.md)
besitzt einen eigenen Quellen- und CLI-Nachweis. Sie qualifiziert keine realen
Werkzeugpakete oder Runtimeprofile.

Offen bleiben vollständige reale Paket-/Formatparität, atomare Aktivierung,
Anti-Rollback-Zustand, Rückkehr zur vorherigen Version, das eigene WSL2-Image,
reale neun Werkzeug-/Runtimepakete und deren Plattformabnahme. Ohne bestandene
Qualifikation werden dadurch keine neuen Ausführungen freigegeben.
