# Ursprünglichen Runtime-Prozessabschluss abgleichen

Der Owner kann bei einem unterbrochenen verwalteten Einzelauftrag den
ursprünglichen Prozessabschluss ausdrücklich prüfen. Die Runtime verwendet die
bereits aufbewahrte signierte Abschlussquittung oder liest genau die ursprüngliche
Quittung beim unverändert gepinnten Runner. Das Ergebnis wird zusammen mit dem
Prozessjournal, dem Checkpoint und den Ereignissen atomar gespeichert.

Die Ansicht bietet zwei Aktionen: **Prozessabschluss prüfen** und
**Prozessabschluss prüfen und Auftrag abbrechen**. Die erste Aktion lässt den Lauf
unterbrochen; nach belegtem Abschluss kann der Owner seine Fortsetzung ausdrücklich
anfordern. Die zweite schließt den Auftrag in derselben Transaktion fachlich als
abgebrochen ab. Der tatsächliche Prozessausgang bleibt unverändert erhalten:
Auch ein physischer Exit-Code 0 wird dadurch nicht zu einer physischen Abbruchquittung.
Ein unbekannter Exit-Code wird als unbekannt angezeigt.

## API

`POST /api/v2/orders/{order}/runs/{run}/managed-processes/{attempt}/reconcile`
verwendet die bestehende Owner-Anmeldung und liefert `Cache-Control: no-store`.

Die Anfrage `ReconcileOwnerManagedProcess` enthält ausschließlich
`expected_run_revision`, `expected_request_sha256` und `finish_cancellation`.
Die Antwort `OwnerManagedProcessReconciliation` enthält `run` und
`process_attempt`. Der Renderer übermittelt keine Prozessresultate, Quittungen
oder Beweishashes als Autorität.

Eine erstmalige Änderung verlangt den exakten unterbrochenen Lauf ohne Workerlease
und genau dessen noch ungeklärten ursprünglichen Prozessversuch. Die Runtime prüft
die unabhängig gespeicherte lokale Controlleridentität. Historische Signaturen,
Plan, Eingabemanifest, bestätigte Laufdefinition, SQL-Arbeitskopienachweis und
ursprünglicher Request bleiben gebunden. Ein inzwischen gesperrtes Ziel, eine
gesperrte Umgebung oder abgelaufene frühere Leserechte verhindern die reine
Abschlusshistorie nicht. Ein erforderlicher entfernter Quittungsabruf verlangt
weiterhin die exakte ursprüngliche kryptografische Runneridentität.

Der Abgleich startet keinen Prozess, erneuert keine Lease, aktiviert keine
Umgebung und erteilt keine Datei- oder Ergebnisleserechte. Ein noch nicht
abgeschlossener oder nicht verifizierbarer Prozess bleibt ungeklärt. Workflows
verwenden ihre gesonderte Wiederherstellung.

## Verlorene Antwort

Die Oberfläche speichert die ursprüngliche Absicht vor dem POST. Bei verloren
gegangener Antwort erfolgt kein automatischer zweiter POST. **Ursprünglichen
Abgleich erneut prüfen** verwendet ausschließlich dieselbe gespeicherte Anfrage.
Das funktioniert auch nach Neuladen, wenn die Änderung bereits als bestätigt im
Laufdetail steht. Ein exakter erfolgreicher Replay liest den ursprünglichen
Journalbeleg ohne neuen Runneraufruf oder zweite Checkpointänderung. Eine spätere
Laufänderung macht die frühere Revisionsbindung ungültig.

**Lokale Abgleichreferenz entfernen** entfernt nach ausdrücklicher Bestätigung nur
die lokale Anfrage. Gespeicherter Prozess, Abschlussquittung und Auftrag bleiben
erhalten. Nicht lesbare lokale Anfragen sperren neue POSTs, bis ihre lokale
Referenz ausdrücklich entfernt wurde.

## Prüfgrenze

Der erste private Prüfstand vom 31.10.2026 enthält 16 Store-/Runtimefälle und
20 gezielte SDK-/Journal-/SSR-Fälle. Die bisherigen sieben Completionregressionen
bestehen aus derselben versiegelten Runtime-Testdatei. Synthetische Runnerbelege
dieser Tests ersetzen keine physische Abnahme.

Die 34 geprüften Dateien sind inzwischen in den aktuellen Checkout übernommen.
Der zusammengeführte Frontendstand besteht 651 Tests, Typprüfung, Web-Build und
die Konsistenzprüfung aller 737 generierten Verträge. Ein tatsächlich gemounteter
StrictMode-Browsertest verwendet die echte Oberfläche, den SDK und das
Anfragejournal mit ausdrücklich simuliertem Backend. Er belegt verlorene
Antwort, Neuladen und ausdrücklichen exakten Erfolgsreplay. Er ist keine
physische oder installierte App-Abnahme. Die vollständigen 71 SQLite- und
71 PostgreSQL-Fälle bestehen mit dem versiegelten Originaltestprogramm. Dieser
historische Quellenstand ist getrennt von den nachfolgenden Lesekontexten belegt.

Am 01.11.2026 besteht außerdem eine tatsächliche native Cancelprobe: Sie wartet
auf den Gaststart und fordert dann über `Store.cancel` den Abbruch an. Die Runtime
gleicht die ursprüngliche signierte Abschlussquittung ab; ein exakter Replay
erzeugt keinen weiteren Prozessversuch. Der Auftrag ist fachlich `Canceled`, der
Versuch `ReconciledCompleted`. Der physische Ausgang bleibt `canceled` mit
unbekanntem Exit-Code. Stop, zwölf Abschlussbestätigungen, Reaping, geschlossene
Pipes und die Bereinigung sind belegt. Die Diagnosemarker erteilen keine
Produktberechtigung. Der unabhängige Nachcheck prüft auch die ursprüngliche
Ed25519-Signatur. Der Quellenumfang und die Grenzen stehen im
nativen Prüfbeleg (development artifact omitted from public sources).

Die erste PostgreSQL-Matrix und der erste Linux-Build wurden durch den Neustart
unterbrochen; ihre Teilbelege bleiben erhalten. Die getrennten neuen Generationen
sind abgeschlossen. Alle 71 neuen PostgreSQL-Testdatenbanken sind entfernt und
ihre Abwesenheit ist nachgeprüft. Die aktuelle installierte App-Abnahme, frische
Ergebnislesekontexte nach Stopp und der frühere HTTP409 beim Eingabe-Missing-Abruf
waren zu diesem Prüfzeitpunkt noch offen. Die
[Lesekontexte mit tatsächlichem Stopp und Ergebnisabruf](MANAGED-RESULT-CONTEXT.md)
sind inzwischen integriert und separat geprüft. Installierte App und frühere
Missing409-Ursache bleiben offen.
