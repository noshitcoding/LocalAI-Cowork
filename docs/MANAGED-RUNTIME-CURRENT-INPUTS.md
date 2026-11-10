# Frische Prüfung eines vorhandenen Runtime-Pakets

`InstalledPackage::open` erwirbt unter Linux neue, schreibgeschützte lokale
Handles für die tatsächlich vorhandenen signierten Eingaben und alle neun
entpackten Rollen. Erwarteter Manifest-Hash, unabhängiger Vertrauensschlüssel,
Plattform und restriktive Freigabegrenze sind verpflichtend. Es gibt keinen
Konstruktor aus Quittungen, gespeicherten Geräte-/Inodewerten oder Einzelpfaden.
Der Loader repariert keine Bytes oder Rechte und stellt keine frühere
Publikations-, Prozess-, Lösch- oder Ausführungsautorität wieder her.

Der feste Versionsname bindet die exakten signierten Manifestbytes. Die aktuelle
Verzeichnisstruktur, jedes Mitglied, Dateityp, Rechte, Größe, Link und Inhalt
werden gegen dieselben bereits geprüften Archive kontrolliert. Gemeinsame
Paket-, Expansions-, Pfad- und tatsächliche Deskriptorgrenzen gelten weiterhin.
Die bestehenden nativen Archivprüfer werden wiederverwendet. Gleichzeitige
Prüfungen desselben Pakets teilen eine Sperre für dessen Dateilesepositionen.
Der opaque Paketwert hält alle neu erworbenen Originalhandles bis zum Drop.

`FloorAnchor` hält separat geöffnete Identitätshandles der ursprünglichen
Policy. Er hält deren exklusives Writer-Lock nicht dauerhaft. Erst
`admit_new_start` öffnet und prüft den aktuellen Stand erneut. Sein
`NewRuntimeAdmission` hält das exklusive Lock und eine Referenz auf das Paket
bis zum Drop. Währenddessen kann keine regelkonforme Policy-Anhebung erfolgen.
Ein Austausch durch eine andere gültige Policy am selben Pfad wird für die
bereits lebende Bindung abgewiesen. Eine neue Anhebung sperrt weitere Zulassung;
behaltene Bytes und Historie werden dadurch nicht auf ein anderes Paket gelenkt.
Diese Komponente ersetzt weder Akteursrechte noch physische Qualifikation.

Der gebundene private Prüfstand (development artifact omitted from public sources)
umfasst 95 Paketquellen, 21 erfolgreiche Prüfschritte, 20 Windows- und 24
Linuxfälle sowie beide strikten Clippy-Prüfungen. Vier native Prüfprogramme und
14 tatsächliche Dateisystem-Suiten sind archiviert. Die neue UID0-Probe auf
einem eigenen 8-MiB-tmpfs prüft alle neun Rollen, falsche echte Paketidentität,
gemischte gültig signierte Rollen, bytegleichen Inode-Ersatz, kurzlebiges
Policy-Lock, Anhebung und tatsächlichen Policy-Austausch. Normales Aushängen
und Entfernung sind belegt. Die beiden getrennten ext4-SIGKILL-Matrizen mit je
acht Fällen bleiben enthalten. Übersprungene Fälle gelten nicht als bestanden.

Dieser Stand ist ausschließlich ein privater API-Teilstand. Die verpflichtende
Bootstrap-/Profil-/Spawnbindung und eine intern abgeleitete, schreibgeschützte
Runtime-Sicht fehlen noch. Die Publikationswurzeln bleiben privat mit 0700;
die gVisor-Anforderung öffentlich durchquerbarer Werkzeugvorfahren bleibt
unverändert. Reale Werkzeugpakete, Aktivierung, Updates, Rückkehr,
WSL-Installation und vollständige Plattformabnahme sind damit nicht geliefert.
