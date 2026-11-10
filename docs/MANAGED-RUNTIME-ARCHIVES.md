# Begrenzte Vorprüfung und Extraktion signierter Runtime-Archive

Der neue Paketpfad liest ein Archiv ausschließlich aus einem ursprünglichen,
vollständig geprüften `StagedPackage`. `InspectedArchive` hält diese native
Verwahrung weiter. Ein exportiertes Inventar oder JSON-Bericht kann sie nicht
rekonstruieren. Archivprüfung und Extraktion erteilen keine Ausführungsrechte.

Unterstützt wird ein ausdrücklich begrenztes unkomprimiertes USTAR-Profil mit
Verzeichnissen, regulären Dateien und symbolischen Links. Das Headerlayout und
die Prüfsumme folgen der [GNU-Dokumentation des Standardformats](https://www.gnu.org/software/tar/manual/html_node/Standard.html).
Hardlinks, Geräte, FIFOs, Sparse-Dateien, PAX-/GNU-Erweiterungen und andere
Archivformate werden mit einem konkreten Fehlergrund abgewiesen. Der spätere
Paketproduzent muss reale Inhalte in dieses unterstützte Profil überführen;
daraus folgt keine allgemeine Ubuntu-/Werkzeugarchiv-Kompatibilität.

Die unabhängig gewählten Installationslimits begrenzen die Zahl der Einträge
auf höchstens 200.000, die Summe regulärer Dateibytes auf höchstens 12 GiB und
Pfad-/Linktext auf höchstens 16 MiB. Geprüft werden Headerprüfsumme, Oktalfelder,
Blocklänge, Dateigrößen, Nullpadding und das vollständige Archivende. Nach zwei
Nullblöcken sind nur begrenzte Nullbytes erlaubt. Verkettete Archive und fremde
Trailer werden abgewiesen. Inhalte werden mit einem festen 64-KiB-Puffer gelesen.

Alle Pfade sind relative, eindeutige UTF-8-Pfade ohne Punktkomponenten,
Backslashes, Laufwerksnotation oder Steuerzeichen. Eltern müssen zuvor als
Verzeichnisse angelegt sein. Ein Link kann deshalb niemals Elternpfad einer
Datei sein. Relative Linkziele dürfen die Gastwurzel nicht verlassen. Absolute
Linkziele behalten ausschließlich ihre Bedeutung innerhalb der späteren
Gastwurzel; weder Prüfung noch Entpacker folgen ihnen auf dem Host.

Die Vorprüfung berechnet für jede Datei SHA-256 und ihren ursprünglichen
Datenbereich. `copy_file` liest nur diesen Bereich aus dem weiterhin gehaltenen
Original, prüft Identität, Länge und Inhaltsprüfsumme erneut und folgt keinem
Archivpfad. Bei einem Fehler bleiben bereits geschriebene Bytes unqualifiziert;
ein aufrufender Installer darf sie nicht veröffentlichen.

Der Linux-Entpacker verlangt UID 0 und einen neuen privaten `StagingArea`-Zielpfad
auf dem unabhängig begrenzten Verwaltungsdateisystem. Er erstellt zuerst
Verzeichnisse und reserviert sämtliche regulären Dateiinhalte physisch, bevor
die erste Dateinutzlast kopiert wird. Ursprüngliche Datei- und Verzeichnishandles
bleiben dabei offen. Linkobjekte entstehen erst
nach allen Dateikopien. Endgültige Dateien besitzen Modus 0444 oder 0555,
Unterverzeichnisse 0555; die private Zielwurzel bleibt 0700. Archivseitige
Sonderrechte und fremde UID/GID werden abgewiesen.

`ExtractedArchive` hält die ursprüngliche Quelle, Zielwurzel und für jeden
Zieleintrag genau ein ursprüngliches Handle. Reguläre Dateien werden über ihren
ursprünglichen Schreibdeskriptor befüllt und ohne Lücke auf einen gehaltenen
Lesedeskriptor überführt. Verzeichnisse behalten ihre ursprünglichen `Dir`-Handles;
Links erhalten eigene `O_PATH | O_NOFOLLOW | O_CLOEXEC`-Deskriptoren. Linktexte werden
direkt am gehaltenen Link gelesen. Device/Inode dienen ausschließlich zum Vergleich
mit diesen lebenden Originalen. Sie ersetzen keine native Verwahrung. Die Linux-
[open-Dokumentation](https://man7.org/linux/man-pages/man2/open.2.html) beschreibt
die Bindung eines offenen Deskriptors unabhängig vom späteren Pfadnamen.

Eine eigene `TargetDescriptorPolicy` begrenzt die Zahl gehaltener Zielobjekte.
Vor dem ersten Zielmitglied werden die tatsächlich benötigten Deskriptorplätze
sowie begrenzter temporärer Spielraum reserviert. Nicht verfügbare Plätze oder
eine überschrittene Policy führen zu `target_descriptor_budget`; ein numerischer
Inode-Cache ersetzt fehlende Handles nicht. Die Bibliothek verändert keine
Prozess-, systemd- oder globalen WSL-Limits. Ein dedizierter Installationsprozess
muss seine erforderlichen Ressourcen zuvor ausdrücklich erhalten. Die Formatgrenze
von 200.000 Einträgen bleibt bestehen; eine bestandene Vorprüfung verspricht keine
verfügbaren OS-Deskriptoren. Drop schließt ausschließlich die eigenen Handles und
löscht keine unbekannten Pfade.

Die temporäre Reserve berücksichtigt die tatsächlich gelockte Bibliothek:
`cap-primitives 4.0.3` hält je aktivem `ReadDirInner` zwei Deskriptoren. Für die
rekursive Verifikation werden daher mit geprüfter Ganzzahlarithmetik
`2 * (maximale Pfadtiefe + 1) + 32` Plätze eingeplant. Darin sind Zielwurzel und
fester Helferraum enthalten. Die Regression verwendet einen gültigen signierten
Baum mit 40 Verzeichnisebenen und einer Datei. Bei ausschließlich im eigenen
CLI-Unterprozess gesetztem `RLIMIT_NOFILE=140` besteht die alte `Tiefe + 32`-
Vorprüfung, scheitert aber erst nach 41 erzeugten Zielmitgliedern. Der korrigierte
Pfad lehnt denselben Fall vor dem ersten Mitglied ab. Der tatsächliche
fehlgeschlagene Altbeleg bleibt erhalten und wird nicht als Produktpass gewertet.

`verify()` prüft Quelle, vollständiges Zielinventar,
Eigentümer, Dateirechte, Hashes und unveränderte Linktexte. Fremde Mitglieder,
bytegleicher Objektersatz und manipulierte Inhalte erzeugen keine erfolgreiche
Bereitstellung. Fehler führen nicht zum Löschen oder Übernehmen unbekannter
Verzeichnisse. Windows bietet die Vorprüfung; Linux-Dateisystemextraktion gehört
in den Linux-Verwaltungsdienst der späteren eigenen WSL-Distribution.

Die native CLI stellt beide ausdrücklichen Pfade bereit:

```text
cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID PLATFORM --stage ABS_PRIVATE_PARENT STAGE_NAME INPUT_BYTE_LIMIT --inspect-archive base_rootfs MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES
cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID linux_x86_64 --stage ABS_PRIVATE_PARENT STAGE_NAME INPUT_BYTE_LIMIT --extract-archive ABS_PRIVATE_TARGET OUTPUT_NAME base_rootfs MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES
```

Der Bericht nennt geprüftes Rollenarchiv, Eintragszahl, expandierte Größe und
den tatsächlichen Extraktionsstatus. `installed`, `qualified` und
`execution_authority` bleiben `false`. Andere Rollen können auf dieselbe Weise
ausgewählt werden; ein nicht unterstützter Komponenteninhalt wird abgewiesen.

Der frische Main-Prüfstand vom 01.11.2026 bindet die tatsächlich integrierte
16-Dateien-Union und 78 Paketquellen. Alle 2.313 geschützten Workflow-,
UI-, Client-, Vertrags-, Storage- und Manifestdateien blieben bei Integration
und Paketprüfung unverändert. Vier tatsächlich ausgeführte Windows-/Linux-
Prüfprogramme, sämtliche Logs und alle vier privaten Dateisystemfälle sind
mit ihren ursprünglichen SHA-256-Werten zurückgelesen. Die früheren privaten
Freeze- und RED/GREEN-Nachweise bleiben unverändert; der Main-Lauf hat einen
eigenen Bericht. Der frühere Staging-Mainnachweis bleibt separat erhalten.

Der gebundene Entwicklungsnachweis (development artifact omitted from public sources)
umfasst 17 Windows- und 20 Linuxfälle, beide Clippy-Prüfungen und je vier
Builderfälle. 14 unabhängig mit Python `tarfile` erzeugte USTAR-Vektoren werden
innerhalb tatsächlich signierter Paketfixtures geprüft. Darunter sind 13
Ablehnungsfälle für gefährliche oder nicht unterstützte Strukturen.

Vier zusätzliche ausdrücklich gestartete Linux-Fälle verwenden je einen eigenen
privaten Mount-Namespace mit einem auf 8 MiB begrenzten tmpfs: tatsächliches
`ENOSPC` beim Paket-Staging sowie UID-0-Extraktion einschließlich Aufruf des
originalen hashgeprüften Linux-CLI-Artefakts. Geprüft werden Quittung, Dateibytes,
Links, schreibgeschützte Modi, Objektersatz und manipulierte Ergebnisse. Der neue
Verwahrungstest löscht Originalnamen tatsächlich, ersetzt Datei, Verzeichnis und
Link und prüft die weiterhin offenen Originalinodes per `fstat`. Er belegt deren
Schließen nach Drop. Ein eigener CLI-Unterprozess mit tatsächlichem
`RLIMIT_NOFILE=48` scheitert vor dem ersten Zielmitglied; das Limit des Elternprozesses
bleibt unverändert. Diese begrenzte Deskriptorprüfung ist keine dauerhafte
Ressourcenreservierung gegenüber fremden Threads im Verwaltungsprozess.
Der vierte Fall prüft den tiefen gültigen Baum mit dem separaten knappen
Unterprozesslimit und unverändertem Elternlimit.
Die Mounts werden regulär ausgehängt und die eigenen Fixturepfade entfernt.
Dies sind Komponenten- und CLI-Nachweise unter WSL, keine native Linux-Hardware-,
persistente Blockgeräte-, Gastisolations- oder installierte App-Abnahme.

Offen bleiben unter anderem reale Werkzeugpakete und Formatkonvertierung,
versionsgebundene Veröffentlichung innerhalb des Installers, Anti-Rollback,
atomarer Versionswechsel, Reparatur/Rückkehr, eigene WSL-Distribution und
Plattformqualifikation. Paket C und der Gesamtumbau sind damit nicht abgeschlossen.

Die ergänzte [gemeinsame Neun-Rollen-Vorbereitung](MANAGED-RUNTIME-INSTALLATION.md)
verwendet dieselben Prüf-, Reservierungs- und Kopierstufen. Ihr eigener Nachweis
belegt Gesamtbudgets und die vollständige Reservierung vor jeder Rollennutzlast;
Installation und Aktivierung bleiben getrennte, offene Schritte.
