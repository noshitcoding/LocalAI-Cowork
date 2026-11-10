# Dateien über einen bestätigten Auftrag importieren

Die Übertragung einer Datei speichert zuerst ihre geprüften Bytes. Sie verändert
keine Arbeitsdatei und startet keinen Lauf. „Import prüfen“ bindet danach Upload,
Zielpfad, gegebenenfalls die beobachtete Zielversion und ausgewählte Eingaben an
einen gespeicherten Ausführungsplan.

Erst die ausdrückliche Planbestätigung startet den Import in einer neuen eigenen
Arbeitskopie. Im Bestätigungsmodus folgt die Freigabe der genauen Dateiaktion.
Originaldateien, vorherige Ergebnisse und der Chatentwurf bleiben erhalten. Der
Import benötigt kein Modell. Auch das Ersetzen einer beobachteten Datei betrifft
ausschließlich die neue Kopie; ihr vorheriger Hash muss zur eingefrorenen Eingabe
passen. Uploadbytes werden bei Vorbereitung, Bestätigung und Dateieffekt geprüft.

Der ursprüngliche Auftrag, die Eingabeauswahl und eine offene Bestätigung bleiben
beim Neuladen gespeichert. Wiederholtes Bestätigen liefert die ursprüngliche
Startquittung. Verwerfen sperrt dieselbe Absicht auch dann, wenn eine frühere
Vorbereitungsantwort verloren ging. Eine beschädigte oder abweichende lokale
Referenz wird nicht durch eine neue Absicht ersetzt.

„Datei hinzufügen“ bleibt auch neben einem kopierten Ergebnis verfügbar. Soll der
Import auf einem bestimmten Ergebnis aufbauen, zuerst ab dessen Antwort
verzweigen. Nach dem Import kann wiederum ab seiner Antwort verzweigt werden;
der neue Chat erhält eine unabhängige Arbeitsgrundlage mit beiden Dateiständen.
Portable Eingaben behalten dabei ihre sichtbaren `input-0001`-Ordner.

Die öffentlichen Endpunkte für Vorbereitung und Rücknahme liegen unter
`/api/v2/execution-plans/v2/file-imports`. Bestätigen, Originalquittung, Kopiecustody
und Ergebnisspeicherung nutzen dieselben Wege wie andere PlanV2-Aufträge.
Neue Direktstarts über `/api/v2/file-imports` verlangen einen bestätigten Plan;
vorhandene alte Anfragen bleiben über ihre ursprünglichen Belege wiederholbar.
Interne native Kompatibilitätshelfer bleiben für frühere Daten und Prüfungen
erhalten. Es gibt keine neue Queue und keine Migration; Schema112 bleibt bestehen.

Die getrennte lokale Prüfung umfasst fünf identische SQLite-/PostgreSQLfälle,
vier authentifizierte HTTPfälle, fünf Wiederherstellungsregressionen und den
tatsächlichen Browserpfad mit Reload, Freigabe, Download, Quittungsreplay,
verworfenem Folgeplan und erneuter Ergebnisverzweigung. Der behaltene Testdaemon
wurde nach geprüftem nativen Shutdown aktualisiert; Sicherung und Erhalt aller
398 Tabellen sind unabhängig geprüft. Die gemeinsamen 722 Verträge wurden mit
dem Workflow-Agent erzeugt; der aktuelle gemeinsame Frontendlauf besteht mit
533 Prüfungen in 104 Dateien.

Nachweise: `.local/remediation-20261024/import-plan2-*.json` und Screenshots 69–71.
Der getrennte Enterprise-Vertrag umfasst nun 90 Definitionen, 15 Wurzeln und 21
Routen. Seine Metadatenkompatibilität erteilt keine Ausführungsrechte. Gemeinsame
Team-/Geräte-/Programmzulassung und die vollständige Produktabnahme bleiben Teil
des laufenden Auftrags.
