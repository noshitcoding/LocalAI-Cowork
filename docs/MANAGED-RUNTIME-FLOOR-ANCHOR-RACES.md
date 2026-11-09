# Originalbindung des laufenden Versionsschutzes

Der private Nachfolger korrigiert zwei getrennte P2-Befunde des ursprünglichen
Current-Inputs-Loaders. Der eingefrorene Vorgänger und seine Originalbelege
bleiben erhalten; seine grünen Komponentenprüfungen sind kein Race-Fixnachweis.

Bei der Anchor-Akquisition werden die neu geöffneten Owner-/Slot-Dateien jetzt
gegen das ursprüngliche vollständige Binding geprüft. Der ursprüngliche
ReleaseFloor wird nach der Akquisition erneut kontrolliert. Der Anchor hält
die ursprünglichen lokalen IDs und eine anfängliche restriktive Mindestsequenz
samt Manifestbindung. Er behält weiterhin unabhängige Lesehandles und sperrt
die Policy nicht dauerhaft exklusiv.

Bei neuer Admission muss auch der frisch geöffnete ReleaseFloor seine tatsächlichen
Parent-/Verzeichnis-/Owner-/Slot-IDs gegen diesen Anchor nachweisen. Ein Wechsel
A→B→A kann damit keine Sperre für B zurückgeben, obwohl die letzte Namensprüfung
A sieht. Ein gültiger niedrigerer Stand auf denselben Slot-Inodes kann zudem
das anfängliche Minimum dieses lebenden Anchors nicht unterschreiten.

Die aktuelle private Qualifikation (development artifact omitted from public sources)
bindet 96 Quellen, 22 erfolgreiche Gates, 20 Windows-/24 Linuxfälle, beide strikten
Clippy-Prüfungen, fünf tatsächliche Programme und 15 Dateisystem-Suiten. Das fünfte
Programm ist ausdrücklich ein Unit-Test-ELF mit cfg(test)-Haltepunkten, keine
unveränderte Produktions-/CLI-Binärdatei. Zwei tatsächliche RED-Rennen wurden
zuerst mit dem ursprünglichen Fehlerverhalten beobachtet; danach bestehen dieselben
beiden Szenarien und eine zusätzliche Initialminimum-Kontrolle. Nur die Steuerung
der Unterbrechungszeitpunkte ist instrumentiert. Dateien, Inodes, Signaturen,
vollständiges Open/check_candidate und Sperren verwenden tatsächliche Betriebssystemoperationen.
Der Acquisition-Test erzeugt offengelegt gültige lokale Ersatzrecords mit echten
neuen IDs im selben Verzeichnis; Admission B entsteht über initialize/advance.
Normales Aushängen und Fixture-Entfernung sind belegt.

Diese API-Korrektur liefert keine verpflichtende Bootstrap-/Profil-/Spawnbindung,
öffentliche Runtime-Sicht, Aktivierung oder reale Werkzeugpakete. Stop, Historie
und bestehende Arbeit benötigen keine frische Floor-Admission. Neustart-, Backup-,
Schlüsselrotations- und vollständige Plattformnachweise bleiben getrennt offen.
