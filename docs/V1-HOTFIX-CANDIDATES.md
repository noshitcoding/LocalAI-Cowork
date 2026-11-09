# Private V1-Hotfixkandidaten

Die beiden Kandidaten werden ausschliesslich in getrennten privaten Quellen geprueft.
Sie veraendern weder den V1-Arbeitsstand noch Referenzarchive, Live-Instanzen,
Desktop-Sidecars oder Releasekanaele. M0 und der Gesamtneubau bleiben offen.

## Crew-Berechtigungsgrenze

Der Crew-Kandidat (private development evidence; omitted from public sources)
verweigert alte Crew-Ausfuehrung ohne exakt gewaehltes `bypass`, solange eine sichere
Einzelaktionsfreigabe fehlt. Thread- und uebergebene Modi werden vor der nativen
Kontextvorbereitung geprueft. Fehlende alte Config/Payloads duerfen weder globale
Fallbacks noch andere Engines nutzen. Unsichere alte Zeitplaene werden pausiert.

Der abgegrenzte Beleg umfasst 94 TypeScript-, sieben Python- und elf Daemontests,
sowie drei separat reine Tauri-Guardunits. Die TypeScriptproben enthalten reale
Eintrittsmethoden mit ersetzten Abhaengigkeiten; die Tauri-Units sind keine
Native-Command-Integration. 39 andere Daemontests wurden dabei nicht ausgefuehrt.

Die anschliessende vollstaendige Frontend-Typpruefung (private development evidence; omitted from public sources)
fand eine echte DTO-Verwechslung im neuen Crewchat-Pfad: gespeicherte
Threadkonfigurationen besitzen kein `authorizedPaths`. Offline-Installation bestand;
der Web-Build wurde nach dem Typfehler nicht ausgefuehrt. Die separate Folgekorrigierung (private development evidence; omitted from public sources)
liest Dateigrants ausschliesslich aus uebergebenen Runtimeoptionen; gespeicherte
Threadverzeichnisse erzeugen keine neuen Grants. Vollstaendige Typpruefung und Web-Build
bestehen, ebenso 37 gezielte Tests im regulaeren App-Vitest/jsdom. 1.097 kombinierte
Quellen und 15.189 Dependency-Dateien bleiben vor/nach identisch; 44 Distdateien sind
gehasht. Der Node/jsdom-Enginehinweis bleibt dokumentiert. Die 37 Tests ueberschneiden
sich mit den vorherigen 94 und werden nicht addiert. Ein kompletter V1-/Nativebuild
ist weiterhin nicht nachgewiesen.

## Office-Fallback

Der getrennte Office-Beleg (private development evidence; omitted from public sources)
bindet beide alten direkten Fallbackdefinitionen an explizite Agentenwerkzeuge,
getrennte Lese-/Schreibfreigaben und Verbotsregeln. Das Arbeitsverzeichnis erteilt
hier keine zusaetzlichen Rechte. Alle Zielpfade werden vor dem Lesen der Quelle
und vor der ersten Ausgabe geprueft. Ein unklarer Werkzeugfehler wird nicht wiederholt.

17 Office-Policypruefungen und drei Crew-Eintrittsregressionen bestehen unter Windows.
Die Proben erzeugen echte synthetische PPTX-Dateien und pruefen ZIP-Struktur/Inhalt.
Der alte Fehler wurde mit leeren Pfadgrants, gesperrten Werkzeugen und Denyregeln
reproduziert; dieselbe Anfrage wird im neuen Pfad verweigert und erhaelt ein vorhandenes
Ergebnis. Symlink-, Traversal-, Groessen-, exakte Filegrant- und Mehrzielproben
verwenden nur private Testdaten. Ein erster Fehlversuch wegen fehlerhafter
Testbeobachtungen bleibt separat erhalten.

CrewAIs BaseTool-Klassencontainer wird in diesen Officeproben ersetzt; der echte
Pfadresolver, das Officewerkzeug, Pydantic und das installierte python-pptx laufen.
Word/DOCX, Microsoft-Office-COM, Linux und ein race-sicherer nativer Handle-Sandboxpfad
sind damit nicht abgenommen. Die breitere alte Werkzeug-Policy und Modellreparaturschleife
sind ebenfalls kein Gegenstand dieses begrenzten Fixes.

Der Office-Unified-Diff ist ein UTF-8-Reviewartefakt. Die Ausgangsdateien besitzen LF,
die geprueften Windows-Kandidaten CRLF. Quellenbundle und Datei-SHA256 sind fuer die
exakten Bytes massgeblich; eine Patchanwendung muss die dokumentierten Zeilenenden
beruecksichtigen.

## Vor einer Verteilung

Der vollstaendige private Frontendbeleg (private development evidence; omitted from public sources)
erhaelt alle vorherigen Kandidaten und Fehlversuche. Der kombinierte Stand braucht noch
Tauri-/Rust-Build, passende Plattform- und Integrationspruefungen sowie das bestehende
Releaseverfahren. App-MSRV, Runtimeartefakte und Installer sind getrennte Voraussetzungen.
Unmarkierte beschaedigte Crew-Altzeilen lassen sich vor dem Laden ihrer gespeicherten
Config nicht sicher als Crew erkennen. Die gespeicherte Freigabe eines Zeitplans muss
zum Pruefen gelesen werden; Null-Binding-/Provider-/Processproben sind kein pauschaler
Nachweis ohne jede Entschluesselung.

Keine Veroeffentlichung oder Verteilung ist erfolgt oder durch diese Testbelege freigegeben.

## Vollstaendige Frontendtests

Der erste komplette Standard-Vitestlauf hielt 767 bestandene und einen fehlgeschlagenen Test fest. Der alte CrewPanel-Positivtest erwartete eine Ausfuehrung im Modus Standard. Ein gesonderter reiner Test-Folgestand waehlt fuer den Positivfall explizit bypass und prueft die Standard-Verweigerung am echten UI-Klick einschliesslich ausbleibender neuer Readiness-/Execute-Aufrufe. Die Produktquellen bleiben identisch zum Crew-/Office-/Typkandidaten.

Alle 769 regulaeren Frontendtests, die Typpruefung und der Web-Build bestehen. Die 20 gerichteten CrewPaneltests sind enthalten und werden nicht hinzugezaehlt. 1.097 Quellen und der eingefrorene Elternstand sind unabhaengig per Hash nachgeprueft, ebenso die 44 Distdateien. Neue Dependency-Dateien wurden in diesem Folgelauf nicht vollstaendig vor/nach gehasht; die fruehere 15.189-Dateien-Pruefung bleibt ihrem damaligen Stand zugeordnet. Native-/Plattform-/Releaseabnahme und vollstaendiges M0 bleiben offen.
