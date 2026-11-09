# Antwort erneut erzeugen

Die Regeneration einer Modell- oder unterstützten Workflowantwort legt einen unabhängigen Chat an und zeigt
dort vor dem Start die gemeinsame Ausführungsprüfung. Erst „Plan bestätigen und
Arbeitskopie starten“ bestätigt den Auftrag. Modell, Dateien, Rechte und Grenzen
sind in dieser Prüfung sichtbar. Die Regeneration benutzt die gewählte aktuelle
Chatkonfiguration und den ursprünglichen Benutzerprompt.

Bei einer Workflowantwort bleiben die ursprüngliche veröffentlichte Version,
ihr Definitionshash und ihre typisierten Eingaben gebunden. Eine inzwischen
veröffentlichte neuere Version wird nicht ausgewählt. Der neue Auftrag erhält
einen frischen, an seine eigene Arbeitskopie gebundenen Plan; der alte Planhash
wird nicht als Berechtigung übernommen. Die Prüfung zeigt Workflowname und Version.

Der übernommene Verlauf endet vor diesem Benutzerprompt. Die ersetzte Antwort
gelangt nicht als Kontext in den neuen Auftrag. Der Ursprungsauftrag, seine
Ergebnisse und sein Entwurf bleiben erhalten. Die neue Dateikopie entsteht über
denselben bestätigten PlanV2-Pfad wie bei einem normalen Einzelauftrag.

Der ursprüngliche Verzweigungsschlüssel bleibt bis zur dauerhaften Übergabe an
das Startjournal des neuen Chats erhalten. Eine verlorene Verzweigungsantwort,
ein Neuladen oder ein Speicherfehler erzeugt keine Ersatzabsicht. Eine vorhandene
abweichende oder beschädigte Startanfrage wird nicht überschrieben. Nach der
Übergabe übernimmt die bestehende Planprüfung Bestätigung, Originalquittung und
verbindliches Verwerfen. Der vorbereitete Prompt bleibt beim Verwerfen erhalten.

Unterstützt werden lokale Kopie-Workflows ohne Programmumgebung oder externe
Connectoren und mit portablen Rechten. Alte Definitionen mit konkreten
`workspace:<alter-auftrag>`-Rechten werden vor dem Verzweigen gestoppt. Diese
Rechte benötigen einen ausdrücklich deklarierten und vertrauenswürdig aufgelösten
portablen Ressourcenbezug; ein automatisches Umschreiben veröffentlichter
Definitionen würde ihre Bindung verändern.

Vorhandene native Eingaben werden über einen [gebundenen Originalbezug](WORKFLOW-INPUT-SOURCES.md)
mit Laufkennung und Datenhash wiederverwendet. Die neue Startanfrage enthält die
Referenz statt erneut serialisierter Zahlen. Die Prüfung zeigt den exakten
kanonischen Originaltext. Auch `9007199254740993` bleibt dadurch unverändert.
Fremde Projekte, eine andere Version, ein abweichender Hash oder eine zugleich
übermittelte Inlineeingabe werden abgewiesen. Ein älteres lokales Startjournal
ohne Originalbezug bleibt lesbar; unsichere Zahlen darin werden weiter gesperrt.

Dateiaktionen und andere nicht unterstützte Ausführungen werden nicht als
Modellauftrag wiederholt. Ihr Prompt kann im passenden Auftrag erneut verwendet
werden. Bei übernommenen Antworten wird die ursprüngliche Ausführung vor dem
Verzweigen geprüft.

Der bisherige `Client.createRun`-Aufruf bestätigte intern einen Legacy-Plan
automatisch. Ein tatsächlicher Browsertest reproduzierte diesen unerwünschten
Start; der eigene Testlauf wurde abgebrochen. Die Regeneration verwendet diesen
Aufruf nicht mehr. Der erste Regenerationsschnitt benötigte keine neuen DTOs.
Der anschließende Originalinputbezug ergänzt zwei öffentliche Datentypen und
optionale Felder; ausgelassene Felder erhalten bisherige Request-/Preflightbytes.
Eine Migration ist nicht erforderlich.

Die getrennten lokalen Nachweise liegen unter
`.local/remediation-20261024/regeneration-*.json` und den zugehörigen Logs.
Zusätzliche Nachweise unter `workflow-regeneration-*.json` prüfen den tatsächlichen
Browserpfad mit neuerer veröffentlichter Version, Reload desselben Plans,
ausdrücklicher Bestätigung, eigener Kopie, erhaltenem Quellauftrag/Entwurf,
Originalquittung und verworfenem Folgeplan. Der native Testanbieter ruft dabei
kein reales Modell auf. Zehn gezielte Workflowfälle und der gemeinsame Lauf mit
546 Frontendfällen bestehen; Typprüfung und Webbuild sind erfolgreich.
Der anschließende Originalinputschnitt besteht mit drei identischen SQLite-/
PostgreSQL-Fällen, 23 nativen Snapshotregressionen, fünf HTTPfällen, sieben
Vertrags-/Kompatibilitätsfällen und 28 gezielten Client-/UI-Fällen. Der gemeinsame
Endlauf hat 559 Frontendfälle in 108 Dateien; 724 Verträge, Typprüfung und Webbuild
sind konsistent. Der tatsächliche Browser zeigt und verarbeitet die große
Ganzzahl exakt, einschließlich Reload, Bestätigung, Originalquittung und
verworfenem Folgeplan, vollständig ohne Modell. Während der Planprüfung erscheint
nur die gebundene Eingabe; der separate Editor für einen späteren Auftrag bleibt
bis zum Abschluss oder Verwerfen ausgeblendet. Die zugehörigen Nachweise heißen
`workflow-input-source-*.json`.
Beide lokalen Updates erhalten alle 398 Tabellen bei unverändertem Schema112.
Diese getrennten Prüfungen ersetzen keine vollständige Team-/Geräte-/Runtime-
oder Produktabnahme.
