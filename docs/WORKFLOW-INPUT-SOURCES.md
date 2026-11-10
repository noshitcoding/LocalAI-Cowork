# Ursprüngliche Workflow-Eingaben wiederverwenden

Die bestätigte lokale Workflow-Regeneration kann vorhandene Eingabedaten nativ
wiederverwenden. JavaScript muss große Ganzzahlen dabei weder lesen noch erneut
als JSON-Zahl senden. Die ursprüngliche Version und ihr Definitionshash bleiben
gebunden; Rechte, Ziel, Arbeitskopie und Plan werden für den neuen Auftrag frisch
geprüft. Dies betrifft die Wiederverwendung vorhandener nativer Eingaben.

`GET /api/v2/runs/{run}/workflow/input-snapshot` ist ein ausschließlich lesender,
Owner-authentifizierter Endpunkt. `WorkflowInputSnapshot` enthält den genauen
kanonischen JSON-Text und `WorkflowInputSource {run_id, sha256}` sowie Projekt,
Workflow, Version und Definitionshash. Die Referenz enthält Datenidentität und
erteilt keine Ausführungs- oder Dateirechte.

Eine `WorkflowRunRequest` kann im bestätigten PlanV2-Pfad `input_source` anstelle
von `inputs` angeben. Die native Auflösung prüft denselben Projektumfang, die
ursprüngliche unveränderliche Version und den exakten Datenhash. Inlineeingaben
und ein alter `expected_binding` dürfen nicht gleichzeitig vorliegen. Normale
Legacy-Vorprüfungen und deren automatische Startpfade weisen eine solche Referenz
ausdrücklich ab. Fehlende neue Felder bleiben beim Serialisieren ausgelassen;
alte Start- und Preflightbytes behalten ihre ursprünglichen Hashes.

Der bestätigte `WorkflowPreflight` bindet denselben `input_source`. Client und UI
verwenden für Integrität und Anzeige den exakten Abschnitt aus `canonical_json`.
Ein neu gehashter Plan mit auf `9007199254740992` gerundeter Eingabe wird auch dann
abgewiesen, wenn `JSON.parse` die gleiche gerundete Zahl anzeigen würde. Beim
Commit liest der native Resolver die Quelle erneut, bevor eine Kopie entsteht.
Die gemeinsame Queue übernimmt die bestätigten nativen Daten. Ein bereits
angenommener Commit liefert weiter seine Originalquittung zurück.

Qualifiziert sind drei Fälle auf SQLite und mit demselben Artefakt auf drei
getrennten PostgreSQL-Datenbanken, 23 Snapshotregressionen, fünf authentifizierte
HTTPfälle, sieben Vertragsfälle, 28 gezielte Client-/UI-Fälle sowie der gemeinsame
Lauf mit 559 Frontendfällen. Zwei vorhandene externe Snapshotfälle bleiben in
diesem Standardlauf ausdrücklich ignoriert. Der reale Browserpfad umfasst exakte
Anzeige, Reload, Bestätigung, modellfreie Ausführung, unveränderten Ursprung und
Entwurf, Originalquittung und Verwerfen ohne Lauf. Schema112 bleibt unverändert;
alle 398 Tabellen sind über unabhängig geprüfte Sicherungen beim Update erhalten.

Der getrennte Enterprise-Server liefert diesen Quellenvertrag im unveränderlichen
Bundle `b26930323954804895a67b025cc5d7975d21af2950a72f7904490b29df4bf2fd`
mit 92 Definitionen, 16 Roottypen und 22 Routen. Native Bundle-, Offlineimage- und
beide authentifizierten Metadaten-API-Prüfungen bestehen. Dies erteilt keine
Enterprise-Ausführungsrechte. Konkrete alte Workspace-Rechte, portable Ressourcen,
Programm-/Connector-/Team-/Geräteadapter und die Gesamtproduktabnahme bleiben
gesonderte Arbeiten.
