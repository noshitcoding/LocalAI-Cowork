# Gemeinsame Workspace-Inhaltsübertragung

Format 1 ist ein plattformunabhängiger Inhaltsvertrag für die Snapshot-Grenze des
V2-Umbaus. Der aktuell integrierte Produktpfad exportiert bereits dauerhaft
gespeicherte **Ergebnissnapshots** eines Owner-Laufs. Private Originaldateien werden
dabei nicht neu gelesen. Die Planbindung und Übertragung von Eingaben vor dem
Dispatch sowie die Veröffentlichung einer Arbeitsversion sind noch offen. Der
private, dauerhaft wiederaufnehmbare Empfängerkern ist zusätzlich implementiert;
`ExecutionPlan.boundary.input_snapshot_bound` bleibt derzeit `false`.

## Vertrag und Autorität

`WorkspaceTransferManifest` enthält Formatversion, sortierte Verzeichnisse,
sortierte Dateien, Gesamtbytezahl und den kanonischen SHA-256 des Inhaltsmanifests.
Dateien enthalten relativen Pfad, Größe, vollständigen SHA-256 und eine lückenlose
Folge von Chunks mit Offset, Größe und SHA-256. Volle Chunks sind 4 MiB groß;
der letzte Chunk darf kleiner sein. Eine leere Datei hat den SHA-256 der leeren
Bytefolge und keine Chunks. Leere Verzeichnisse bleiben explizit erhalten.

Native Pfade, Dateihandles, Device-/Inode-Werte, FileBroker-Ressourcenidentitäten,
Zugangsdaten und Ausführungsrechte gehören nicht in dieses Manifest. Ein Hash
erteilt keinen Zugriff. Der konkrete Owner-Order-/Run-Pfad wird serverseitig gegen
den gespeicherten Lauf und seinen ursprünglichen Snapshot geprüft. Ein Empfänger
benötigt später eine eigene, aktuelle, an den bestätigten Plan gebundene Autorität.

Die gemeinsame Rust-Domäne prüft Hash, Format, Größen, Reihenfolge, vorhandene
Eltern, Chunkabdeckung und plattformübergreifende Pfadkonflikte. Relative Traversals,
Windows-Gerätenamen einschließlich hochgestellter COM-/LPT-Ziffern, ADS-/Laufwerks-
Syntax, Steuerzeichen, reservierte interne Namen, Abschluss-Punkte/-Leerzeichen
und kollidierende Schreibweisen werden abgewiesen. Der private Empfängerkern
schreibt zusätzlich exklusiv in einer privaten Stage, weist Symlinks/Reparse-Points
ab und lässt tatsächliche Dateisystemkollisionen an exklusiven Creates scheitern.
Diese Prüfung ersetzt keine native lokale Identitäts- oder Lockprüfung.

Grenzen: 10.000 Dateien und Verzeichnisse zusammen, 64 MiB je Datei, 512 MiB
Gesamtinhalt, 2 MiB serialisiertes Manifest und maximal 65 Pfadkomponenten.
Nicht portable historische Dateien bleiben über den bestehenden historischen
Download lesbar; dieser neue Transferpfad benennt sie nicht stillschweigend um.

## Integrierte Owner-Endpunkte

- `GET /api/v2/orders/{order}/runs/{run}/workspace/transfer`
  erzeugt das deterministische portable Manifest aus bestätigten, unveränderlich
  aufbewahrten Ergebnisbytes.
- `GET /api/v2/orders/{order}/runs/{run}/workspace/transfer/chunk`
  liest mit `path`, ausgerichtetem `offset` und `expected_sha256` genau einen
  Chunk der dort registrierten Quelldatei. Eine globale Blobkennung ist kein
  Dateiselektor. Der Chunk bekommt Binärantwort, exakte Länge und SHA-256-Header.

Beide Wege erfordern Owner-Authentisierung im Header. Sie verwenden zusammen mit
Uploads die bestehende Begrenzung auf zwei aktive Übertragungen. Die Freigabe
bleibt im tatsächlich laufenden blockierenden Broker-Aufruf, auch wenn der
HTTP-Aufrufer verschwindet. Die wiederholte Leseanfrage liefert dieselben
Inhaltsbytes; sie startet keine Ausführung und übernimmt keine Ergebnisse.

Der TypeScript-Client stellt `workspaceTransferManifest` und
`downloadWorkspaceTransferChunk` bereit. Er prüft den kanonischen Manifesthash,
liest nur deklarierte Offsets und kontrolliert Länge und Chunkhash vor Rückgabe
eines Blobs. Es gibt keine impliziten Wiederholungsversuche. Ein Empfänger muss
vor Nutzung oder Veröffentlichung außerdem den vollständigen Dateihash und die
Vollständigkeit des gesamten Manifests prüfen.

## Privater Empfang und Arbeitskopie

`crates/broker/src/workspace_receive.rs` stellt `WorkspaceReceiver` bereit. Sein
Aufrufer muss ein leeres, geschütztes Verzeichnis auf einem vorab reservierten,
begrenzten Runtime-Volume und eine unabhängig aufgelöste Plan-/Empfängerbindung
liefern. Die Rust-Struktur `TransferBinding` ist ausdrücklich kein Zugangstoken.
Es gibt dafür noch keinen öffentlich erreichbaren Upload- oder Start-Endpunkt.

Der Empfang benötigt vier feste Dateien: unveränderliches Intent-Journal,
Packdatei, Fortschrittsbits und OS-Lock. Inhalt landet an aus dem geprüften
Manifest abgeleiteten festen Offsets. Die Packdatei wird vorab alloziert; Linux
verwendet direkt `fallocate` ohne Sparse-/glibc-Ersatzpfad. Windows verwendet
`FileAllocationInfo`, kontrolliert die tatsächliche Allokation und lehnt Sparse-
oder komprimierte Dateien ab. Diese Allokation reserviert die Empfangsdatei;
sie ersetzt keine Begrenzung des gesamten Arbeits-, Log- oder Cache-Dateisystems.

Jeder Chunk wird vor dem Schreiben nach Länge und SHA-256 geprüft. Erst nach
dem Flush seiner Bytes wird sein Fortschrittsbit geschrieben und ebenfalls
geflusht. Beliebige Reihenfolge und explizite Wiederholung nach verlorener Antwort
liefern dieselbe deterministische Quittung. Bereits quittierte Bytes werden vor
Wiederholung erneut geprüft und bei Beschädigung nicht stillschweigend repariert.

Beim Wiederöffnen müssen die vom Aufrufer erwartete Plan-ID, Planhash,
Empfänger-ID, Generation und das vollständige Manifest unverändert passen.
Verzeichnis, Lock, Packdatei und Fortschrittsdatei behalten ihre lokalen nativen
Identitäten. Windows speichert dafür Volume und vollständige 128-Bit-Datei-ID;
Unix verwendet Device/Inode. Diese Werte bleiben ausschließlich im privaten
Journal und sind keine plattformübergreifenden Ressourcenkennungen. Ein
unvollständiger Create ohne fertiges Intent-Journal wird nicht automatisch adoptiert.

`verify` hält den exklusiven OS-Lock, prüft sämtliche quittierten Chunks sowie
unabhängig davon jeden vollständigen Dateihash und gibt erst dann eine
`VerifiedTransfer`-Capability zurück. `materialize` schreibt ausschließlich in
eine weitere leere private Verzeichnis-Capability, legt jede Datei exklusiv an,
reserviert sie, prüft ihre Inhalte erneut und gibt erst nach vollständigem Erfolg
`ReceivedWorkspace` zurück. Teilweise Materialisierung bleibt privat; sie wird
weder zurückgegeben noch automatisch bei einem späteren Versuch überschrieben.
Der Runtime-Adapter muss diese Arbeitskopie anschließend als Inhaltsversion
veröffentlichen, ihre Kapazität und Lebensdauer verwalten und vor Dispatch die
aktuelle Ausführungsautorität erneut prüfen. Der Empfängerkern startet kein Programm.

Windows flusht Dateidaten. Die Crash-Dauerhaftigkeit der Verzeichnismetadaten ist
damit nicht als Windows-Hosttransaktion qualifiziert. Die produktive Linux-Runtime
in WSL und auf nativem Linux braucht ihre eigene Volume-/Lifecycle-Abnahme.
Die verwendeten Allokationsschnittstellen sind in der
[Microsoft-Dokumentation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_allocation_info)
und der [Linux-Dokumentation](https://man7.org/linux/man-pages/man2/fallocate.2.html)
beschrieben.

## Prüfung und noch offene Integration

Gezielte Fälle liegen in `crates/domain/tests/workspace_transfer.rs`,
`crates/broker/tests/workspace_transfer.rs`,
`crates/broker/tests/workspace_receive.rs`,
`services/daemon/tests/workspace_transfer.rs` und
`packages/client/src/workspace-transfer.test.ts`. Die Brokerfälle rekonstruieren
mehrteilige Binärdateien in beliebiger Abrufreihenfolge nach Änderung der lebenden
Quelldatei und Wiederöffnung des Brokers. Beschädigte aufbewahrte Bytes, fremde
Pfade, falsche Prüfsummen und ungültige Offsets werden abgewiesen.

Der Zwei-Verzeichnis-Fall exportiert einen echten gespeicherten Broker-Snapshot,
empfängt zuerst den letzten Chunk, öffnet den unabhängigen Empfänger erneut und
materialisiert trotz zwischenzeitlich geänderter Quelldatei die ursprünglichen
Bytes. Weitere Fälle prüfen falsche Plan-/Empfängerbindung, ersetzte native
Dateiidentität, beschädigte quittierte Bytes, verlorenen Fortschrittsflush,
unabhängige Vollhash-Prüfung, unbekannte oder vorhandene Zielbäume und leere
Snapshots. Unix prüft zusätzlich Symlinks und Hardlinks.

`tools/v2_rebuild/qualify_workspace_receiver.py` reproduziert diese eng begrenzten
Entwicklungsprüfungen, speichert Logs und unsigned Testartefakte und kontrolliert,
dass sich die einbezogenen Quellen während der Prüfung nicht ändern. Mit
`--wsl-distro Ubuntu-24.04 --volume-proof` wird zusätzlich ein eigener temporärer
16-MiB-ext4-Loop angelegt. Der unprivilegierte Negativtest muss dort einen
32-MiB-Snapshot vor Intent/Quittung ablehnen. Der Wrapper entfernt nur sein eigenes
Volume und bestätigt Unmount und Loop-Detach. Das ist kein Ersatz für den
Runtime-/Hardware-Nachweis und keine neue Runtime-Freigabe.

Die lokale Auswahl/Erfassung ist inzwischen unter [Ausführungseingaben](EXECUTION-INPUTS.md)
angebunden; sie sendet noch keine Eingabebytes an Runner. Offen bleiben die
bestätigte Planbindung dieser Auswahl, an Plan und Empfänger gebundene
Übertragungsrechte, zentrale Reservierung der tatsächlichen Zielkapazität,
atomare Veröffentlichung der privaten Arbeitskopie, Dispatch erst nach
vollständigem Snapshot und getrennte Ergebnisübernahme mit Baseline-/Konfliktreview.
Der neue Inhaltsvertrag qualifiziert keine Runtime und ersetzt diese Schritte nicht.
