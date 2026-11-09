# Ergebnisabruf nach einem physischen Stopp

Ein beendeter Prozess und ein fachlich abgebrochener Auftrag sind verschiedene
Zustände. Nach **Prozessabschluss prüfen** bleibt der Auftrag unterbrochen. Wenn
die ursprüngliche signierte Quittung den tatsächlichen Abschluss belegt, kann der
Owner das aufbewahrte Ergebnis ausdrücklich als eigene lokale Version abrufen.
**Prozessabschluss prüfen und Auftrag abbrechen** schließt den Auftrag dagegen
fachlich ab; dieser Pfad erteilt keine neuen Inhaltsrechte.

Originaldateien und die bestätigte Eingabekopie bleiben erhalten. Der Abruf
veröffentlicht erst nach vollständiger Prüfung eine lokale Ready-Version. Eine
Wiederholung liefert dieselbe Version. Die Übernahme in eine Arbeitskopie benötigt
weiterhin ihren eigenen Vergleich und eine ausdrückliche Bestätigung.

## Aktuelle Rechte und privater Transport

Vor dem Abruf prüft die Runtime den aktuellen Auftrag, den bestätigten Plan,
Controller, Runner, das registrierte Ziel, die Umgebung und die tatsächlichen
Kopie- und Ordnerrechte. Anschließend holt sie über
`POST /v2/managed/results/{attempt}/context` einen frischen signierten Lesekontext.
Dieser bindet die ursprüngliche Launch-Anfrage, Abschlussquittung und aktuelle
Runtime-Instanz. Eine neue Kennung und höchstens 30 Sekunden Gültigkeit begrenzen
den Kontext; die ursprüngliche Aufbewahrungsfrist gilt zusätzlich.

Ein späterer oder widerrufener Ausführungsfence verhindert damit nicht pauschal
das Lesen eines bereits verifiziert beendeten Ergebnisses. Der Kontext erneuert
weder Ausführungsepoche noch Workerlease und startet keinen Prozess. Ungeklärte
Versuche und fachlich abgebrochene Aufträge bleiben gesperrt. Rechte und die
ursprünglichen Fristen werden auch nach asynchronen Antworten und dem Commit
geprüft. Windows und Linux verwenden zusätzlich eine Uhr, die Schlafzeit mitzählt;
ein Neustart übernimmt keine alten kurzlebigen Lesefähigkeiten.

Der Produktclient verwendet ausschließlich den neuen Format-2-Lesepfad. Alte
Format-1-Signaturbytes bleiben für ihren bisherigen eng gebundenen Pfad erhalten;
es gibt keinen automatischen Rückfall auf alte Leserechte.

## Prüfung und Grenzen

Der aktuelle Prüfbeleg (development artifact omitted from public sources) trennt den eigenen
Main-Prüflauf, die PostgreSQL-/HTTPS-/Uhrregressionen und die tatsächliche native
Probe. Letztere startet einen gVisor-Gast, fordert über `Store.cancel` den Abbruch
an, gleicht die ursprüngliche Ed25519-Quittung ab und ruft danach ausdrücklich die
41 Byte umfassende Ergebnisdatei ab. Original und Eingabekopie bleiben erhalten;
der genaue Erfolgsreplay erzeugt keinen weiteren Prozess oder Modellaufruf.
Ein unbekannter Gast-Exit-Code bleibt unbekannt.

Die Quellen sind integriert. Die installierte App, dauerhafte Ergebnis-/Volume-
Freigabe, ausdrückliche Ergebnisübernahme und die frühere Missing-HTTP409-Ursache
haben weiterhin eigene offene Abnahmen. Ein erfolgreicher physischer Durchlauf
belegt keine allgemeine Stabilität. Der erste vor dem Gaststart fehlgeschlagene
Test bleibt zusammen mit seiner gezielten Fixture-Korrektur erhalten.
