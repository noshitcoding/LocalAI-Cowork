# Privater Empfang von Ergebnissen

Der native FileBroker besitzt jetzt einen getrennten Empfängerkern für Ergebnisse
der eigenen Runtime. Er startet kein Programm, übernimmt keine Dateien und
veröffentlicht selbst keine Ergebnisversion. Die angebundene
[Runtime und SQL115-Veröffentlichung](MANAGED-RESULT-VERSIONS.md) prüfen die
[frischen Inhaltsrechte](MANAGED-RESULT-READS.md) unabhängig davon.

Der Empfänger bindet ursprünglichen Plan, Lauf, Prozessversuch, Eingabemanifest,
Abschlussquittung, Output-Attestation und tatsächliche native Controllerherkunft.
Diese Angaben sind Custody-Kontext; ihre Konstruktion allein erteilt keine Rechte.
Der private native Record enthält zusätzlich die Identitäten des Container- und
Transferverzeichnisses. Er gehört ausschließlich in das Controllerjournal.

Der anfängliche lokale Speicherbereich lässt höchstens acht Empfangsstages zu.
Jede Packdatei umfasst höchstens 512 MiB plus begrenzte Metadaten. Der Broker
zählt das Stage-Verzeichnis dauerhaft vor der Packallokation. Angefangene,
fertige und verwaiste Creates bleiben auch nach Drop und Neustart belegt.
Unbekannte Verzeichnisse werden nicht automatisch übernommen oder freigegeben.
Dies begrenzt die vom vertrauenswürdigen Broker angelegten Empfangspacks; es ist
keine Qualifikation einer harten Quote für das gesamte Hostdateisystem.

Der bestehende `WorkspaceReceiver` alloziert die Packdatei vorab, quittiert
verifizierte Chunks erst nach Flush und prüft beim Wiederöffnen seine nativen
Dateiidentitäten. `verify` kontrolliert zusätzlich vollständige Dateihashes.
Erst danach kann eine `VerifiedManagedResultStage` deklarierte Blöcke lesen.
Der ursprüngliche Deskriptor und Lock bleiben dabei erhalten; jeder zurückgegebene
Block wird erneut gehasht. Originalordner sind keine Ersatzquelle.

Wiederaufnahme verlangt den unabhängig gespeicherten erwarteten Record.
Ein zweiter Broker erhält während aktiven Empfangs keinen Besitzerlock.
Ein kopiertes gleichnamiges Verzeichnis mit denselben Dateibytes ersetzt die
ursprüngliche native Custody nicht. Abgewiesene Kapazitäts- und unbekannte
Custody-Anfragen erzeugen keine zusätzlichen Stage-Lockdateien.

Der getrennte Prüfstand (development artifact omitted from public sources) besteht 17 echte
Windows-Dateisystemfälle und 18 auf WSL/Linux, jeweils einschließlich acht neuer
Stage-Fälle und bestehender Receiver-/Transferregressionen. Sechs tatsächlich
ausgeführte Testartefakte und die Quellinputs sind gebunden. Ein im unabhängigen
Review gefundener unbegrenzter Lockdatei-Zuwachs bei abgewiesenen Anfragen wurde
korrigiert und auf beiden Systemen geprüft. Die erste WSL-Logerfassung blieb wegen
eines fehlenden Ergebnisabschnitts zurückgewiesen; die vollständige Erfassung
verwendet getrennte Pipes und die unverändert geforderte Fallzahl.

Der anschließende Versions-Prüfstand (development artifact omitted from public sources)
erweitert diesen eingefrorenen Stand um exakte Create-Wiederaufnahme und eine
bis über den SQL-Commit gehaltene echte Verifikationsfähigkeit. Er besteht
19 Windows- und 20 WSL/Linux-Fälle mit je zehn Stage-Fällen. SQL-Journal,
Runtime-Downloader und atomare Veröffentlichung sind getrennt
qualifiziert (development artifact omitted from public sources).

Offen bleiben explizite Kapazitätsfreigabe nach dauerhaftem Retirement sowie
Vorschau und bestätigte Übernahme. Ein Drop löscht keine aufbewahrten Ergebnisbytes.
Die laufende App ist durch diesen separat geprüften Kern nicht aktualisiert worden.
