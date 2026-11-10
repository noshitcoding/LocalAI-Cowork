# Dokumente und Bildbearbeitung in LocalAI Cowork V2 vervollständigen

**Historischer Audit.** Der inzwischen vereinbarte Umfang umfasst auch Fachansichten,
OCR, lokale/Cloud-Bildmodelle und alle V2-Ziele. Aktueller Fortschritt und offene
Umsetzung: [Implementierungsstand vom 24.10.2026](DOCUMENT-MEDIA-IMPLEMENTATION-2026-10-23.md).

Stand der Prüfung: **20.10.2026**, Quellversion **2.0.0-alpha.7**. Geltungsbereich sind die aktuelle V2-App, ihre Dokumentwerkzeuge, gemeinsame Oberfläche und angebundenen Ausführungswege.

PDF, LaTeX, Bilder, PowerPoint, Word und Excel besitzen bereits eine umfangreiche gemeinsame Engine. Der nächste Entwicklungsschritt ist, diese Engine zuverlässig in der gesamten App nutzbar zu machen, bekannte Fehler zu beheben und die Ergebnisse mit echten Laufzeiten und ausgelieferten Paketen abzunehmen. Eine Neuentwicklung der sechs Formatadapter ist dafür nicht erforderlich.

Der neue gemeinsame Pfad besteht aus `document_read`, `document_write` und `image_edit`. Er ist im lokalen Einzelchat angeschlossen. Die Dokument-Workbench verwendet weiterhin den älteren `office`-Pfad; Workflows, Team, Offlinebetrieb, Android und Geräteagenten besitzen noch keine gleichwertige Anbindung der neuen Werkzeuge. Vorhandene Office-Funktionen dieser Wege bleiben bei der Vervollständigung zu berücksichtigen.

## Zielumfang der Vervollständigung

Für jedes Format gilt als Ziel: Datei auswählen oder importieren, Inhalt lesen und untersuchen, unterstützte Änderungen ausführen, Ergebnis als neue Datei veröffentlichen, Vorschau prüfen und Datei herunterladen. Zugriffsmodus, Dateiversion, Herkunft und Fehler müssen in allen Schritten nachvollziehbar bleiben.

Der erste abnehmbare Umfang ist Desktop Windows und Linux sowie der persönliche V2-Server mit Weboberfläche. Darauf folgen Workflows und die getrennten Team-, Offline- und Gerätewege. Ein vollständiger WYSIWYG-Ersatz für Microsoft Office, beliebige TeX-Distributionen und generative Bildbearbeitung sind eigenständige Erweiterungen. OCR wird als gesondertes Arbeitspaket geplant; bis dahin muss die App bei Scan-PDFs ihre tatsächliche Lesbarkeit anzeigen.

## Geprüfter Stand pro Format

| Bereich | Bereits im aktuellen Quellcode | Noch zu vervollständigen |
|---|---|---|
| PDF | Text, Seiten und Formularfelder lesen; Metadaten; PNG-Vorschau; Zusammenführen, Aufteilen, Extrahieren, Drehen, Umordnen, Löschen, Wasserzeichen, Formularbefüllung und Metadatenänderung. Erzeugung aus Markdown und Blöcken mit LaTeX oder ReportLab. | Durchgängige Bedienung in der Workbench, echte Konvertierung und Formularanzeige abnehmen, Qualität und Schriftabdeckung des Fallbacks sichern. OCR und redaktionelle Änderung beliebigen vorhandenen PDF-Textes sind nicht implementiert. |
| LaTeX | Eigene TeX-Dateien und explizit mitgegebene Begleitdateien kompilieren; offline arbeitendes Tectonic; Fehlermeldungen mit Datei und Zeile; QA für fehlende Zeichen, Referenzen und übervolle Boxen. Windows-AppContainer sowie Linux-Landlock und seccomp sind angeschlossen. | Projektbedienung mit Hauptdatei, Begleitdateien, Quelltext und PDF verbinden; Cache reproduzierbar prüfen; echte Kompilierung und Isolation als verpflichtende Tests ausführen; Linux-Containerbetrieb qualifizieren. |
| Bildbearbeitung | Zuschneiden, Skalieren, Drehen, Spiegeln, Farbkorrekturen, Graustufen, Weichzeichnen, Schärfen, Text und Formen, Überlagerung, Rand, Formatwechsel, EXIF-Behandlung und Vorschau. | Eigener Bedienpfad mit Vorher/Nachher, Warnungen und bestätigter Ausgabe; Engine- und UI-Formate angleichen; Schriftabdeckung erweitern. Generative Bearbeitung fehlt. |
| PowerPoint | PPTX lesen und untersuchen; Folien mit Text, Bildern, Tabellen, Diagrammen und Notizen erzeugen; Vorlagen; Folien und Objekte gezielt ändern, hinzufügen, löschen oder verschieben. | Gerenderte Folienvorschau in den gemeinsamen Ablauf integrieren; Textüberlauf und Vorlagentreue zuverlässig prüfen; komplexe Präsentationen und echte PowerPoint-/LibreOffice-Ausgabe abnehmen. |
| Word | DOCX lesen; strukturierte Erstellung mit Formatvorlagen, Runs, Tabellen, Bildern, Kopf-/Fußzeilen, Seitenlayout und Inhaltsverzeichnisfeld; DOCX-/DOTX-Vorlagen und Platzhalter; gezielte Absatz- und Tabellenänderungen. | Rundlauf mit komplexer Formatierung und Vorlagen prüfen; Feldaktualisierung und PDF-Ausgabe festlegen; reichhaltige Engine-Funktionen in der Oberfläche nutzbar machen. Mathematik wird im Word-Adapter bisher als LaTeX-Quelltext geschrieben. |
| Excel | XLSX lesen und untersuchen, einschließlich Formeltext und vorhandener Cachewerte; typisierte Zellen, Blätter, Formatierung, Tabellen und Diagramme erzeugen; Bereiche und Blattstruktur bearbeiten. | Rechenweg mit gültigen Ergebniswerten; korrekte Behandlung von Formelreferenzen bei Strukturänderungen; Erhalt vorhandener Zeichnungsobjekte nachweisen oder verlustbehaftete Änderungen begrenzen; echte Excel-/LibreOffice-Abnahme. |

Die Implementierung liegt überwiegend in [document_engine.py](../crates/toolbox/src/document_engine.py) und [image_engine.py](../crates/toolbox/src/image_engine.py). Die Operationslisten und Spezifikationen stehen zusätzlich in deren `help`-Antworten. [DOCUMENT-TOOLS.md](DOCUMENT-TOOLS.md) beschreibt den aktuellen Anschluss an die Runtime.

## Befunde nach Priorität

### P0 Nur Lesen scheitert vor der Werkzeugausführung

Der explizit gestartete Test `read_only_runs_deny_document_writes` scheitert reproduzierbar. Der Lauf endet mit `state: Failed`, `error_code: forbidden` und `steps: 0`, bevor das erwartete Werkzeugergebnis entsteht. Der normale Testlauf überspringt diesen Test durch `#[ignore]`.

[restore_run_workspace](../crates/runtime/src/workspaces.rs) vergleicht ab Zeile 40 den vollständigen gespeicherten Grant mit dem aktuellen Workspace-Grant. Der Nur-Lesen-Lauf enthält `writable: false`; der registrierte Workspace ist schreibbar. Der Broker erlaubt eine solche Einschränkung bereits in seiner Grant-Prüfung. Die Runtime muss Identität, Revision und erlaubte Einschränkung konsistent validieren, ohne Schreibrechte wiederherzustellen.

Abnahme: Ein Nur-Lesen-Lauf mit verwaltetem Workspace kann ein echtes Dokument lesen. Er lehnt `document_write` und `image_edit` ab und erzeugt keine Ausgabedatei. Der ignorierte Test wird aktiviert und um einen tatsächlich erfolgreichen Lesevorgang ergänzt.

### P0 Die CI kann ohne echte Engine und ohne LaTeX grün werden

Die aktiven Runtime-Integrationstests kehren ohne `COWORK_TEST_DOCUMENT_PYTHON` erfolgreich zurück. Der reale LaTeX-Test der Python-Suite kann bei fehlendem Binary oder unvollständigem Cache übersprungen werden. In dieser Prüfung fehlte `size11.clo` im automatisch gewählten Testcache. Die 111 Engine-Testfälle ergeben deshalb **110 bestanden und 1 übersprungen**, keine erfolgreiche aktuelle LaTeX-Kompilierungsabnahme.

[verify.yml](../.github/workflows/verify.yml) richtet die Dokument-Wheels und Tectonic nicht gezielt ein und startet die Python-Engine-Suite nicht ausdrücklich. Die allgemeinen Rust-Tests allein reichen für diese Funktion nicht.

Abnahme: Eigene Windows-/Linux-Jobs installieren die gepinnten Dokumentabhängigkeiten, setzen die benötigten Testvariablen und scheitern bei fehlenden Voraussetzungen. `COWORK_REQUIRE_TEX_TESTS=1` wird für die Rust-Isolationstests gesetzt. Für die Python-Suite wird ebenfalls ein verpflichtender Modus eingeführt. Jeder Bericht weist ausgeführte, fehlgeschlagene und übersprungene Fälle getrennt aus.

### P1 Workbench und Chat verwenden unterschiedliche Dokumentpfade

[OfficeWorkbench](../packages/ui/src/OfficeWorkbench.tsx) prüft ab Zeile 33 `office_available` und führt `kind: office` aus. [OfficeBinaryEditor](../packages/ui/src/OfficeBinaryEditor.tsx) bietet grundlegende Absatz-, Zellen- und Folientextänderungen. Die neue Engine ist dagegen über [document_tools.rs](../crates/runtime/src/document_tools.rs) an normale lokale Chatläufe gebunden. Die gemeinsame Einrichtung macht die umfassenden neuen Funktionen dadurch noch nicht automatisch in der Workbench verfügbar.

Abnahme: Workbench und Modellwerkzeuge verwenden denselben versionierten Dokumentdienst und dieselben Broker-Regeln. Eine UI-Aktion bindet Eingaben und SHA vor der Ausführung, zeigt die konkrete Änderung und veröffentlicht über `Generate`. Vorhandene Office-Läufe und ihre Fingerprints bleiben gültig; gespeicherte Historie wird nicht umgedeutet.

### P1 Formelwerte und strukturelle Excel Änderungen sind unvollständig

`save_xlsx` speichert mit openpyxl und enthält keinen eigenen Berechnungsschritt. `xlsx_inspect` kann vorhandene Cachewerte lesen, deren Aktualität ist damit nicht nachgewiesen. `xlsx_edit` warnt ab Zeile 2791 ausdrücklich, dass Einfügen und Löschen von Zeilen oder Spalten Formelreferenzen nicht umschreibt. Bei vorhandenen Charts und Drawings warnt es ab Zeile 2743 vor fehlendem Erhalt; der zugehörige Test prüft die Warnung und keinen vollständigen Erhalt der Objekte.

Abnahme: Formeltext und geprüfter Ergebniswert werden getrennt ausgewiesen. Ein expliziter Rechenschritt nutzt eine qualifizierte bestehende LibreOffice- oder Microsoft-Excel-Laufzeit und überprüft Ergebnisse. Strukturänderungen aktualisieren abhängige Bezüge korrekt oder werden für betroffene Arbeitsmappen vor der Ausführung als nicht unterstützt abgelehnt. Diagramme, Bilder und Beziehungen erhalten eigene Rundlauftests.

### P1 Vorschau und Einrichtungsstatus bilden die Fähigkeiten unvollständig ab

`documentKind` in [documentFiles.ts](../packages/ui/src/documentFiles.ts) enthält `.tex` nicht in seiner Text-Erweiterungsliste. Mit generischem MIME-Typ fällt TeX deshalb auf `binary` zurück. Die Bildvorschau deckt PNG/JPEG/WebP/GIF ab; die Engine verarbeitet zusätzlich BMP/TIFF. Der PDF-Viewer in [DocumentPreview](../packages/ui/src/DocumentPreview.tsx) hängt von der Browserunterstützung ab, Office benötigt eine gesonderte Konvertierung.

`latexState` und [ToolRuntimeSetup](../packages/ui/src/ToolRuntimeSetup.tsx) erklären installiertes, aber nicht verfügbares LaTeX pauschal mit ausstehender Prozessisolation. Die Isolation existiert inzwischen im Code; tatsächlich kann auch ein Cache-, Probe- oder Konfigurationsfehler vorliegen. `office_convert_available` prüft in [lib.rs](../crates/toolbox/src/lib.rs) Zeile 516 lediglich den konfigurierten Programmpfad.

Abnahme: TeX erscheint als Quelltext, alle unterstützten Bilder erhalten eine normalisierte Vorschau, PDF-Seiten können über den bestehenden Renderer angezeigt werden. Bereitschaft liefert unterscheidbare Gründe wie `missing_runtime`, `cache_incomplete`, `isolation_unavailable` oder `probe_failed`. Eine erfolgreiche minimale Konvertierung qualifiziert die Office-Bereitschaft.

### P1 Gemeinsame Werkzeuge fehlen in weiteren Ausführungswegen

Die lokale [Preflight-Logik](../crates/runtime/src/lib.rs) gewährt Dokumentwerkzeuge ab Zeile 199 automatisch. [workflow_product.rs](../crates/runtime/src/workflow_product.rs) ergänzt sie nicht entsprechend; [workflow_external.rs](../crates/runtime/src/workflow_external.rs) enthält keinen passenden Ausführungszweig. Eine reine Ergänzung der Werkzeugnamen würde das Journal für externe Effekte umgehen.

Teamläufe bieten in [team_toolbox.rs](../services/daemon/src/team_toolbox.rs) weiterhin elf grundlegende `office`-Operationen. [Dockerfile.tools](../deploy/v2/Dockerfile.tools) installiert zwar neue Laufzeitabhängigkeiten, kopiert und startet aber weiterhin den alten Office-Adapter. Offline und Android verwenden ebenfalls eigene Pfade. Der Geräteagent besitzt mit [execution_office.rs](../services/device-agent/src/execution_office.rs) bereits strukturierte OOXML-Erzeugung und Microsoft-Office-Interop; das ist eine vorhandene zusätzliche Implementierung, keine Abnahme der neuen gemeinsamen Engine.

Abnahme: Fähigkeiten werden pro Ausführungsziel ausgewiesen. Ein Agent sieht nur dort tatsächlich ausführbare Operationen. Neue Adapter übernehmen Journal, Freigaben, Dateiversionen, Ergebnisprüfung, Abbruch und Reconcile ihres jeweiligen Ziels. Geräte-Interop bleibt eine ausdrücklich ausgewählte Fähigkeit für vorhandene Microsoft-Installationen.

### P1 Dokumentqualität und aktuelle Paketabnahme fehlen

Im ReportLab-Fallback werden Formeln als Quelltext dargestellt; Schriftabdeckung und CJK-Ausgabe unterscheiden sich vom LaTeX-Pfad. Bildannotation verwendet in `step_text` die eingebaute Pillow-Schrift und warnt bei anderen Schriftsystemen. PPTX besitzt Überlaufwarnungen, aber keine vollständige gerenderte Layoutprüfung. DOCX-Inhaltsverzeichnisfelder müssen im Zielprogramm aktualisiert werden. Diese Grenzen müssen im Ergebnis und vor relevanten Aktionen sichtbar sein.

Python und LibreOffice laufen im aktuellen Dokumentpfad unter Benutzerrechten auf Stage-Kopien. Nur Tectonic besitzt hier Betriebssystem-Isolation. Input-Limits und Audit-Hooks sind vorhanden; sie ersetzen die Prozessgrenze nicht. Qualifikation und UI müssen diese unterschiedlichen Ausführungsbedingungen korrekt abbilden.

[DOCUMENT-TOOLS.md](DOCUMENT-TOOLS.md) kennzeichnet den Stand vom 19.10. ausdrücklich als noch nicht paketiert. Der geprüfte Server-Buildquellenbeleg vom 17.10. enthält die drei neuen Dateien `document_engine.py`, `document_engine.rs` und `document_tools.rs` nicht. Die Versionsnummer `alpha.7` allein belegt deshalb keine Auslieferung der aktuellen Dokumentwerkzeuge.

## Technische Umsetzung

Die bestehenden Adapter und Broker-Verträge bilden die gemeinsame Basis. Vorgeschlagen ist ein interner Dokumentdienst zwischen Runtime und Ausführungsadapter, den Einzelchat, UI-Aktion und Workflow mit ihrem jeweils gültigen Ausführungsnachweis aufrufen. Der Dienst vereinheitlicht Parse/Precheck, Eingabesnapshot, Engine-Auftrag, Belegprüfung, Vorschau und Veröffentlichung. Rechteprüfung und Effektjournal bleiben beim jeweiligen Aufrufer und werden nicht durch einen bloßen UI-Preview-Hash ersetzt.

Der vorhandene `office`-Katalog bleibt kompatibel. Neue UI-Aktionen erhalten einen eigenen versionierten Vertrag. Microsoft-Office-Interop ergänzt die portable Engine bei Bedarf für Layout, Feldaktualisierung oder Berechnung; eine installierte Microsoft-Anwendung wird nicht zur Voraussetzung für grundlegenden Dokument-Support.

LaTeX benötigt eine explizite Projektbeschreibung mit Hauptdatei und Begleitdateien. Die aktuelle Engine nimmt höchstens 16 Eingaben an und verlangt Begleitdateien unterhalb des Hauptdateiordners. Diese Grenze wird in der Oberfläche geprüft. Größere Projekte benötigen später einen versionierten Projektmanifest-Import mit eigenen Mengenlimits. Bibliografie-, Klassen- und Paketbedarf wird mit realen Projekten geprüft, bevor die App entsprechende Kompatibilität zusagt; beliebige fehlende Pakete werden während einer isolierten Kompilierung nicht automatisch nachgeladen.

## Priorisierte Arbeitspakete

Die Größen S, M und L bezeichnen relative Planungsgrößen: ungefähr 1–2, 3–5 beziehungsweise 6–10 Entwicklungstage einschließlich gezielter Tests. Es sind Schätzungen für eine mit V2 vertraute Person, keine Liefertermine. Echte Office-, Linux-, Container- und Geräteumgebungen müssen für die jeweilige Abnahme verfügbar sein.

| ID | Priorität und Größe | Umsetzung und betroffene Bereiche | Abnahme und Abhängigkeit |
|---|---|---|---|
| D01 | P0 · S | Grant-Abgleich in `runtime/workspaces.rs` korrigieren; ignorierten Nur-Lesen-Test aktivieren; erfolgreichen Dokumentlesefall ergänzen. | Lesen funktioniert mit eingeschränktem Workspace, Mutation bleibt verboten, Grant-Identität und Revision bleiben gebunden. Zuerst umsetzen. |
| D02 | P0 · M | Verpflichtende Engine- und Isolationstests, Windows-/Linux-Job, gepinnte Wheels, explizite Tectonic-/Cache-Pfade, klare Skip-Auswertung. Verträge aus `.local/.../INTERFACE.md` in versionierte `docs/` überführen. | Engine läuft real; mindestens ein echtes TeX-Projekt wird isoliert erzeugt und gerendert. Fehlende Voraussetzungen machen den Job rot. |
| D03 | P1 · M | Gemeinsamen Dokumentdienst aus vorhandenem Runtime-Ablauf ableiten; typisierte UI-Aktion mit eingefrorenen Eingaben und Freigabe; bestehenden Broker und Herkunft verwenden. | Identische Operation aus Chat und UI liefert denselben Funktionsumfang; Konflikte, Nicht-Überschreiben und unklare Veröffentlichung sind korrekt behandelt. Nach D01. |
| D04 | P1 · M | Workbench auf D03 aufsetzen; Formatwahl, Quellen, neue Ausgabedatei, Warnungen, Vorschauen und Downloads verbinden. TeX/MIME- und Bildformatlücken schließen. | Je ein PDF-, DOCX-, XLSX-, PPTX- und Bildablauf ist ohne manuelles JSON bedienbar. Historische Dateien bleiben lesbar. |
| D05 | P1 · M | LaTeX-Projektbedienung mit Hauptdatei, Assets, Quelltext, Kompilieren, anklickbaren Diagnosen und zugehöriger PDF-Vorschau; Bereitschaftsgründe und Cache-Reparatur. | Mehrdateiprojekt mit Grafik und Querverweis, absichtlicher Fehler und Korrektur, Neustart sowie fehlendes Paket sind abgenommen. Nach D02/D03. |
| D06 | P1 · L | Excel-Rechenschritt und Ergebnisstatus; Formelreferenzen bei Strukturänderungen; Rundlauf und verlustbezogene Prüfung vorhandener Charts/Bilder. | Reale Ergebniswerte, korrektes Referenzverhalten und definierter Objekterhalt. Nicht unterstützte Fälle werden vor der Änderung erkannt. Nach D02/D03. |
| D07 | P1 · M | PDF-/Word-/PowerPoint-Qualität: Schriftbestand, Fallback-Hinweise, Formularanzeige, Word-Felder, Vorlagen und gerenderte Folien. Bildschrift und Transparenz einschließen. | Referenzdateien öffnen ohne Reparaturdialog, Layout und Inhalte stimmen im festgelegten Umfang. Nach D02, zusammen mit D04/D05. |
| D08 | P1 · L | Workflow-Katalog, Agentenfilter, Toolbudget, Freigaben, Effektjournal, Vorschauen, Ergebnisübergabe und Reconcile für alle drei Werkzeuge ergänzen. | Ein Workflow liest DOCX/XLSX, erzeugt Bericht und Präsentation und verarbeitet ein Bild. Abbruch nach Veröffentlichung führt zu keiner Doppeldatei. Nach D03 und D02. |
| D09 | P1 · L | Gemeinsamen Vertrag auf Team-Container, danach Offline/Geräteagent und Android übertragen; Images mit Adaptern bauen, Capability-Versionen und Provider-Bildweitergabe prüfen. | Pro Ziel separat qualifizierte Funktionen; Isolation und Berechtigungsgrenzen bleiben erhalten. Office-Interop wird separat mit echten Microsoft-Apps getestet. Nach D03/D08. Bei Beginn in zielbezogene Tickets aufteilen. |
| D10 | P1 · M | Frische Windows-/Linux-Desktop- und Serverpakete bauen; Einrichtung, Update, Neustart, Abbruch, Cache, Webvorschau und tatsächliche Modellaufrufe prüfen. Releasebelege binden Quellen und Laufzeitbestand. | Installiertes Paket erfüllt die freigegebene Format-/Zielmatrix. Nach D04–D07; weitere Zielwege erhalten nach D08/D09 eigene Paketabnahmen. |
| D11 | P2 · separat schätzen | OCR für Scan-PDFs und Bilder; danach bei Bedarf generative Bildbearbeitung und umfassendere visuelle Editoren. | Eigener Funktionsvertrag, Provider-/Laufzeitwahl, Qualitätskorpus und Ressourcenlimits. Kein stilles Versprechen innerhalb der Basisabnahme. |

Für D01–D07 und D10 ergibt sich grob ein Umfang von **25–42 Entwicklungstagen** für einen zuverlässig bedienbaren und qualifizierten lokalen/persönlichen Dokumentpfad. D08 und D09 fügen nach dieser ersten Schätzung **12–20 Tage** hinzu; insbesondere die zielübergreifende Gerätequalifikation kann nach Aufteilung mehr Aufwand ergeben. Die Schätzung wird nach D02 anhand der echten Laufzeiten und Referenzdateien aktualisiert.

Die Umsetzung beginnt mit D01 und D02. Anschließend wird D03 als Grundlage für Workbench und Workflow umgesetzt. D04–D07 schließen die lokale Produktfunktion; D10 liefert deren Paketabnahme. D08/D09 erweitern denselben Stand auf weitere Ziele, ohne die erste lokale Lieferung von allen Geräten abhängig zu machen.

## Abnahmematrix

| Bereich | Positiver Endablauf | Erforderliche Fehler und Qualitätsfälle |
|---|---|---|
| Gemeinsam | Import → SHA-Prüfung → Lesen/Inspect → Änderung → neue Ausgabe → Vorschau → Download; im Chat und in der Workbench. | Quelle geändert, Ziel vorhanden, Nur Lesen, Freigabe abgelehnt, Abbruch, Timeout, Neustart zwischen Erzeugung und Veröffentlichung, beschädigte Eingabe, unberechtigter Vorschauabruf. |
| PDF | Mehrere PDFs kombinieren, Seiten teilen/drehen, Formular befüllen, Report aus Markdown mit und ohne LaTeX erzeugen. | Scan ohne Text, verschlüsselte Datei, rotiertes Formular, Wasserzeichen, mehrseitige Tabellen, fehlende Zeichen, gerenderte Formularwerte. Keine Zusage beliebiger Textredaktion. |
| LaTeX | Mehrdateiprojekt mit Grafik, Formel, Querverweis und PDF; Fehler beheben und erneut kompilieren. | Fehlendes Paket, leeres Ergebnis, ungültige Hauptdatei, Asset außerhalb des Projekts, Hostdateilesen/-schreiben, Netz- und Prozessstart, Cacheänderung und harte Beendigung. |
| Bild | Foto skalieren/zuschneiden, transparentes Logo überlagern, Text ergänzen, als neues PNG/WebP speichern. | EXIF-Orientierung, Metadatenentfernung, Unicode, Alphakanal, Farbmodus, BMP/TIFF-Vorschau, Größenlimit und fehlerhafte Datei. |
| Word | Formatierte Vorlage mit Platzhaltern, Tabelle, Bild, Kopf-/Fußzeile und Inhaltsverzeichnis erzeugen und gezielt bearbeiten. | Unterschiedliche Runs, verschachtelte Tabellen, Felder, Seitenumbrüche und komplexe Vorlagen; dokumentierter Umfang für Änderungen/Kommentare und eingebettete Objekte. |
| PowerPoint | Vorlage mit Text, Grafik, Diagramm und Notizen erzeugen; Folien umordnen und Bild ersetzen. | Überlauf, Folien außerhalb des Formats, Gruppierung und Platzhalter, Beziehungen nach Folienlöschung; Rundlauf komplexer Originale. |
| Excel | Mehrblattdatei mit Datum, Zahlen, Formeln, Tabelle und Diagramm erzeugen, ändern, berechnen und prüfen. | Formelergebnis und Cache, Abhängigkeiten nach Umbenennen/Einfügen/Löschen, externe Links, Zeichnungen, Druckbereich, Makrodateien und große Bereiche. |

Visuelle Abnahme verwendet gerenderte Referenzseiten und Folien sowie echte Zielprogramme für die zugesagte Kompatibilität. Reines Wiederöffnen mit derselben Python-Bibliothek belegt Dateistruktur, aber keine vollständige Darstellungstreue. Bildfähige und reine Textmodelle erhalten getrennte Endabläufe; Bilder dürfen nur entsprechend der eingefrorenen Providerfähigkeit und des Zugriffs auf den Lauf weitergegeben werden.

## Tatsächlich ausgeführte Prüfungen

Alle folgenden Ergebnisse beziehen sich auf den lokalen Windows-Quellstand dieser Prüfung. Der Prüfbeleg (development artifact omitted from public sources) enthält Befehle, Ergebnisumfang und SHA-256 der zugeordneten Quellen.

| Prüfung | Ergebnis |
|---|---|
| Python-Dokument-/Bild-Engine im vorhandenen Engine-Venv | 111 Fälle: 110 bestanden, 1 realer LaTeX-Test wegen unvollständigem Cache übersprungen; 100,8 Sekunden. |
| `cowork-runtime --test documents` mit echtem Dokument-Python | 3 bestanden, 1 ignoriert. DOCX-Erzeugung, Freigabe/Autonom, Nicht-Überschreiben, Bildbearbeitung, ReportLab-PDF, PDF-Text und Vorschau werden tatsächlich ausgeführt. |
| Derselbe ignorierte Nur-Lesen-Test explizit gestartet | 1 fehlgeschlagen; Lauf endet vor Werkzeugausführung mit `forbidden`. |
| Dokumentbezogene Domain-/Policy-Tests | 4 Domain-Tests und 1 Policy-Test bestanden. |
| `cowork-broker --test generated` | 3 bestanden; create-only, Herkunft, Reconcile, Rechte und begrenztes Binärlesen. |
| Sechs gezielte UI-Testsuiten | 17 Tests bestanden: Werkzeugzusammenfassung, Vorschauen, Einrichtung, Dateierkennung, Editorentwürfe und Workbench-Helfer. |
| `npm run typecheck` | Bestanden. |
| `npm run contracts:check` | Bestanden; 646 generierte Verträge stimmen überein. Warnungen zu unbekanntem Schemaformat `uint` bleiben bestehen. |
| `python -I tools/toolbox/test_runtime_install.py` | 7 Tests bestanden. |

Ein aktueller Windows-/Linux-Installerlauf, neue Docker-Images, echte Office-Anwendungen, tatsächliche Provideraufrufe und eine neue Betriebssystem-Isolationsabnahme wurden in dieser Prüfung nicht ausgeführt. Die genannten Unit-/Integrationsergebnisse ersetzen diese Produktabnahmen nicht.

## Versionsbindung und Arbeitsbaum

`package.json`, `apps/desktop/src-tauri/tauri.conf.json` und die vorhandenen Alpha-7-Cargo-Metadaten nennen `2.0.0-alpha.7`. Der betrachtete ältere Web-Buildbeleg nennt `built_at: 2026-10-17T14:08:20.298773+00:00`; der zugehörige Server-Quellenbeleg meldet `container_image_built: false` und `dependencies_installed: false`. Sein `package.json`-Hash stimmt nicht mit dem heutigen Quellstand überein. Er qualifiziert die neue Engine daher nicht.

Der gesamte V2-Checkout war zu Beginn ungetrackt. Diese Prüfung ändert keine App-Implementierung und erstellt keinen Commit, Push oder Release. Vorhandene Arbeiten bleiben erhalten.

Die vorgelagerte schreibgeschützte Vault-Abfrage lieferte keinen einschlägigen Projektkontext: Der BM25-Index war nicht verifiziert, und die ersatzweise Suche im Wiki ergab keine passenden Inhalte. Die fachlichen Befunde beruhen auf den hier verlinkten V2-Quellen und den ausgeführten Tests; im Vault wurde nichts gespeichert.
