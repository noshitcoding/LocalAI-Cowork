# Startervorlagen und V1-Pipeline-Import

Die Workflow-Bibliothek erstellt aus einer Startervorlage oder einem ausdrücklich ausgewählten V1-Pipeline-Datensatz einen unabhängigen nativen Entwurf. Danach gelten die bestehenden Draft-Snapshots, Revisionen, Publikationsjournale und Ausführungsfreigaben. Der Import veröffentlicht und startet keinen Workflow.

## Startervorlagen

Die V1-Vorgaben „Research squad“, „Build crew“ und „Review council“ verwenden wie `app/src/stores/crewStore.ts::createStarterCrew` die Stufen Planen, Umsetzen und Prüfen. Ein vorhandenes V2-Modell muss ausdrücklich ausgewählt werden; jede Rolle bindet dessen genaue Revision. Die Vorlage übernimmt das eingegebene Ziel und enthält ausschließlich Artefaktwerkzeuge und Rückfragen. Sie konfiguriert weder Browser, Shell, Ordnerrechte noch externe Recherche. Weitere Werkzeuge müssen anschließend im Editor und bei der Ausführung ausdrücklich ausgewählt werden.

## Unterstützte Pipeline-Daten

Quelle ist der V1-Typ `RpcPipeline` aus `app/src/stores/pipelineStore.ts` und die native Implementierung `pipeline_execute` in `app/src-tauri/src/lib.rs`. Akzeptiert wird ein JSON-Datensatz oder eine Liste mit maximal 100 Datensätzen:

```json
{
  "id": "historical-id",
  "name": "Auswertung",
  "description": "Optionale V1-Metadaten",
  "steps_json": "[{\"tool\":\"ollama\",\"prompt\":\"Analysiere die bereitgestellten Belege.\"},{\"args\":\"Prüfe das vorherige Ergebnis.\"}]",
  "zero_context": false
}
```

- `name` wird Workflowname. `id`, `created_at`, `updated_at` und `description` bleiben historische Metadaten und werden nicht als Laufanweisungen übernommen. Die Beschreibung bleibt vor dem Import sichtbar.
- `steps_json` enthält 1–64 Objekte mit ausschließlich `tool`, `prompt` und `args`. Modellschritte verwenden einen nichtleeren `prompt` vor `args`; strukturierte JSON-`args` werden wie in V1 als Text serialisiert. `web_fetch` und `web_search` geben `args` Vorrang. `plan`/`planner` wählen die tatsächliche Plan-Ausführung. Andere Tool-/Gatewaynamen sind wie in V1 ausschließlich Modellhinweise und erteilen keine MCP- oder Werkzeugrechte.
- Die Reihenfolge bleibt erhalten. Jeder Schritt behält die vorherigen Schritte als Abhängigkeiten. Ohne `zero_context` werden deren Resultate als Kontext übergeben; mit `zero_context` bleiben Resultate ausgeschlossen, während die Ausführungsreihenfolge erhalten bleibt. Fehler stoppen den Workflow entsprechend dem nativen V1-Pfad.
- Ein vorhandenes V2-Modell wird ausdrücklich neu gebunden. Fehlende oder geänderte Modellrevisionen werden abgelehnt; es gibt keine automatische Zuordnung nach Namen oder Endpunkt.
- Modell-/Plan-Schritte besitzen keine Werkzeug- oder Ordnerrechte. Webschritte verlangen Browserrechte in Definition, Rolle, Schritt und aktuellem Lauf. Jede konkrete native Browseraktion benötigt weiterhin die bestehende exakte Freigabe. Die Budgetvorgaben werden an das Kontextfenster des gewählten Modells angepasst und bleiben im Editor sichtbar.
- Dateien müssen UTF-8 sein. Das gesamte Import-JSON darf höchstens 1 MiB groß sein; Schritttexte höchstens 16.000 UTF-8-Bytes. Die konvertierte Definition darf höchstens 990.000 Bytes umfassen, damit der bestehende Draft-Umschlag innerhalb seiner Grenze von einer Million Bytes bleibt. Es werden keine V1-Datenbank, V1-Datei oder laufenden V1-Workflows verändert.

## Web- und Planner-Ausführung

Webschritte frieren die konkrete Fetch-URL oder Suchanfrage als typisierten Schritt ein. Eine reine `Preparation`-Buchung erzeugt den Werkzeugvorschlag ohne Modellkontakt oder erfundenen Tokenverbrauch. Erst die vorhandene Werkzeugfreigabe und der dauerhaft zugelassene Werkzeugauftrag erlauben tatsächliche IO. Ein unbestätigter Auftrag wird nicht erneut gesendet.

`web_fetch` liest Titel, HTTP-Status und begrenzten sichtbaren Seitentext aus einer privaten Playwright-Sitzung. `web_search` verwendet die HTML-Suche von DuckDuckGo und liest bis zu fünf tatsächliche Ergebnisse. Nicht erkennbare Ergebnisse oder eine Anbieter-Challenge führen zu einem sichtbaren Fehler. Dieser V2-Pfad verwendet den eingerichteten Browser; gegenüber dem reinen HTTP-Fetch in V1 können JavaScript und Unterressourcen zusätzliche Browseranfragen auslösen. Die vorhandenen nativen Freigaben bleiben deshalb erforderlich.

Actor-Workflows verwenden ihren eigenen isolierten Chromium-Gast und den vorhandenen privaten Netzwerk-Broker. Jeder Webschritt friert genau einen Ursprung sowie GET/HEAD ein und benötigt die bestehende konkrete Actor-Freigabe. Öffentliche DNS-Ziele auf Port 80/443 bleiben Pflicht; private Netze, IP-Adressen, nicht freigegebene Umleitungen und Unterressourcen werden weiter abgewiesen. Erst der gespeicherte Empfangsbeleg schließt den Schritt ab; ein angenommener oder unbestätigter Auftrag wird nicht erneut versendet. Neu angelegte reine Fetch-/Suchsitzungen werden nach ihrem Ergebnis mit dem ursprünglichen Entfernungsbeleg geschlossen. Dafür muss das entsprechend aktualisierte, fest gebundene private Runner-Image ausdrücklich eingerichtet sein.

Bei der kanonischen sequenziellen Pipeline mit allen vorherigen Schritten als Abhängigkeiten enthält der Actor-Modellkontext die authentifizierten Originalresultate auch oberhalb von 8 KiB. Die vorhandenen Checkpoint- und Modellkontextbudgets gelten unverändert. `zero_context` lässt diese Resultate weiterhin aus; es ergänzt keine Artefakt- oder Dateirechte.

Planner-Schritte verwenden den tatsächlichen V2-Planmodus ohne Werkzeuge. V1-Gatewaynamen wurden bereits in V1 als Kontext für Modellaufrufe behandelt; sie werden weiterhin als inerte Aufgabenlabels erhalten. Der Import verbindet keinen namensgleichen MCP-Server automatisch.

## Historische Persönlichkeiten und Crews

Unter den bereits importierten Konfigurationen lassen sich Persönlichkeitsinhalte, einzelne Crew-Agenten und Crew-Workflows ausdrücklich in neue V2-Libraryeinträge kopieren. Ein aktuelles Modell und dessen genaue Revision werden neu ausgewählt. Vor dem Speichern bleibt die native Kopie editierbar; eine verlorene Bestätigung behält dieselbe Anfrage über Neustarts. Es startet kein Lauf.

Vorhandene historische Versionen bleiben separat auswählbar. Crew-Hierarchien erhalten einen tatsächlichen Manager-Abschlussschritt, der von allen ursprünglichen Arbeitsschritten abhängt. Alte Providerzugänge, Toolrechte, Skill-/Memory-IDs und laufende Zustände werden nicht übernommen. Die ursprünglichen Rollen-/Skilltexte bleiben Anweisungsinhalt; strukturierte Felder, die der ursprüngliche Import nur als Textprojektion aufbewahrt hat, lassen sich daraus nicht verlustfrei rekonstruieren.

Bei neu aufgenommenen Desktop-RPC-Pipelines erhält der historische Katalog zusätzlich die erlaubten Originalfelder einschließlich `args` und `zero_context` als inerte JSON-Quelle. Bereits früher ohne diese Originalfelder aufgenommene Projektionen können fehlende Argumente nicht wiederherstellen und müssen aus dem Original erneut ausgewählt werden.

## Grenzen

Zusätzliche Modell-/Toolreferenzen, untypisierte Schritte, vollständige App-Backups und unbekannte Felder werden abgelehnt. JSON-Argumente mit Schlüssel-, Token- oder Passwortfeldern werden nicht übernommen. Konfigurationen, Schlüssel, Tokens und Gateway-Zugänge sind keine Importfelder; Fehler zeigen keine importierten Werte an. Benutzertext bleibt Benutzertext und darf keine Geheimnisse enthalten. Quellen, Skill-/Memory-Auswahl und aktuelle V2-Sicherheitsregeln können den Kontext gegenüber V1 verändern; der Import verspricht keine identische Modellantwort.
