# Dokumente auslesen und am Original prüfen

Der Starter **Dokument auslesen und prüfen** ist im Main-Quellstand vom
1. November 2026 implementiert. Er verbindet das Lesen einer Quelle, typisierte
Modellvorschläge und eine ausdrückliche menschliche Prüfung. Er erstellt einen
bearbeitbaren Entwurf; Veröffentlichung und Ausführung bleiben eigene Schritte.

**Aktueller Prüfstand:** Die Kontextbegrenzung ist im gemeinsamen Main-Quellstand
korrigiert und unter Windows und Linux mit aufgezeichneten Modellanfragen
geprüft. Der gebundene Main-Browsernachweis (development artifact omitted from public sources)
besteht ebenfalls für einen kurzen Windows-Laufzeitpfad: neuer Entwurf und
Veröffentlichung, ausdrücklich gewählte Version, echte OCR, native Korrektur
und automatische Wiederherstellung der Originalantwort nach Dienstneustart.
Die neue Grenze gilt für eine neu veröffentlichte, ausdrücklich gewählte
Version. Frühere eingefrorene Versionen werden nicht umgeschrieben; ohne die
neue Regel behalten sie ihren bisherigen Chat- und Projektkontext.

1. In der projektbezogenen Ablaufbibliothek **Dokument auslesen und prüfen**
   öffnen. Einen Namen sowie OCR für einen Scan oder vorhandenen Dokumenttext
   wählen. Die lokale Dokumentengine muss für das Ausführungsziel eingerichtet
   sein; OCR benötigt zusätzlich die eingerichteten Sprachdaten.
2. Eine vorhandene Modellrevision ausdrücklich wählen. Der Starter wählt kein
   Modell automatisch. Die vorgeschlagenen Rechnungsfelder sind direkt sichtbar;
   unter **Felder und Prüfregeln anpassen** lassen sich andere Angaben definieren.
   Unbekannte Werte dürfen zunächst leer bleiben, während die spätere Bestätigung
   Pflichtfelder, Kalenderdaten, Geldbeträge und Summen prüft.
3. **Dokumentablauf als Entwurf öffnen** wählen, im Ablaufeditor prüfen und als
   neue Version veröffentlichen. Bei späteren Änderungen müssen Formular und
   typisiertes Extraktionsergebnis zusammenpassen. Die Quelle wird beim Start
   oder über die Eingabezuordnung eines Dateiauslösers verbunden.
4. Im Chat genau die veröffentlichte Version wählen. Unter **Dokument aus
   Arbeitsdateien wählen** eine bereits vorhandene Datei dieses Chats auswählen.
   Der Download prüft den Inhalt und verbindet Dateipfad und Inhaltsbezug gemeinsam.
   Für den Starter muss der Hash dadurch nicht von Hand eingegeben werden.
   Während der Prüfung bleiben Start und manuelle Quellfelder gesperrt. Anschließend
   den Start vorbereiten. Die Dokumentengine
   ausdrücklich wählen und den Plan mit Dateikopie, Modell, Rechten und Grenzen
   prüfen. Erst dessen Bestätigung beginnt OCR und Modellanfrage. Für diesen
   dreistufigen Lese- und Prüfablauf genügt der Modus **Nur lesen**.
5. Die vorgeschlagenen Angaben mit den **Quellen zum Vergleichen** abgleichen.
   OCR zeigt die gespeicherte Seite, den erkannten Originaltext und suchbare
   Wörter mit ihren Fundstellen. Der OCR-Rohwert ist keine Wahrscheinlichkeit.
   Antwortkorrekturen ändern weder die Quelle noch die ursprünglichen Vorschläge.
6. Fehlende oder falsche Werte berichtigen. **Antwort zur Prüfung vorbereiten**
   friert den geprüften Entwurf ein; **Geprüfte Antwort senden** bestätigt genau
   diese Angaben. Folgeschritte erhalten ausschließlich die bestätigten Werte.
   Nach Antwortverlust denselben Vorgang abgleichen. Ein Neustart erhält die
   ursprünglichen Quellen, Vorschläge und bereits bestätigte Antwort.

Die geführte Dateiauswahl verwendet bestehende Arbeitsdateien bis 32 MiB.
Weitere Dateien zuerst über die Arbeitsdateien dieses Chats importieren.
Für bestehende eigene Abläufe erscheint die Auswahl bei direkt verbundenen
Quellfeldern von OCR, Textlesen und Seitenvorschau im aktuellen Arbeitsbereich.
Verschachtelte Eingaben, fremde Ordner und die automatische Fortsetzung langer
Dateilisten gehören noch nicht zu diesem Einstieg. Ein Workflow- oder
Verbindungswechsel verwirft das verspätet eintreffende Ergebnis einer Dateiprüfung.

Der getrennte Main-Nachweis der Dateiauswahl (development artifact omitted from public sources)
bindet das Vierdateien-Delta an den damaligen Main mit 5.069 Quellen vor dem Storage-Tail.
760 Oberflächentests, Typprüfung, Webbuild und 31 tatsächliche Browserprüfungen
bestehen. Der Browser verwendet 15 echte Main-Pfadabhängigkeiten plus einen
Prüfwrapper. Falsche Prüfsummen, direkte Starts während einer Dateiprüfung
und späte Antworten nach Workflowwechsel sind geprüft. Ein neuer
veröffentlichter Ablauf liest das bestätigte Original per OCR, nimmt genau
eine korrigierte Antwort an und stellt nach hartem Dienstneustart denselben
Graphen, dieselbe Quelle und Antwort automatisch wieder her. Die einzige
Modellantwort ist festgelegt; der Nachweis bewertet keine reale Modellgüte.
Die Windows-Pfadgrenze und F71 sind nicht Teil dieser Übernahme.

Der Starter setzt für den Extraktionsschritt **Nur verbundene Eingaben**.
Rollenanweisung und Schrittziel bleiben erhalten; frühere und aktuelle
Chatnachrichten, Projektwissen und zusätzliche Agentvorlagen werden nicht als
weitere Quellen übernommen. Die Regel wird beim Aufbau und beim Laden eines
gespeicherten Schritts geprüft. Sie lässt sich im typisierten Datenbereich pro
Modellschritt ausdrücklich ändern; gewöhnliche Modellschritte behalten den
Modus **Chat- und Projektkontext übernehmen**.
Der erzeugte Entwurf gibt der Extraktionsrolle keine Werkzeuge,
Ordnerrechte oder Delegation. Dokumentinhalt gilt als nicht vertrauenswürdiges
Quellenmaterial. Die native Prüfung braucht selbst kein Modell.

Der Starter liest einen begrenzten Ausschnitt: OCR bis zu 100 Wörter;
Text und OCR führen Fortsetzungsmarker mit. Weitere Wörter oder Seiten werden
noch nicht automatisch nachgeladen. Fehlende Angaben bleiben deshalb sichtbar
und müssen geprüft werden. Nach einem bestätigten Start klappt die unveränderte
Auftragseingabe ein; über **Auftragseingabe und Einstellungen anzeigen** bleibt
sie erreichbar, ohne den Lauf erneut zu senden.

Der gebundene Kontext-Prüfstand vor der Dateiauswahl besteht 749 Oberflächentests,
Typprüfung/Webbuild, 751 frisch erzeugte Verträge und 196 Domänenfälle.
Die vollständige Speicher-Lib besteht unter Windows mit 89, unter Linux mit
88 Fällen; jeweils sechs deklarierte PostgreSQL-Fälle bleiben ausgelassen.
Zusätzlich bestehen je Plattform sechs native OCR-/Extraktionsfälle und zwei
Prüfformularfälle sowie ausgewählte Speicher- und Wiederanlauffälle.
Vollständige lokale Modellanfragen belegen die Kontextgrenze; normale
Modellschritte behalten ihre gültigen zusätzlichen Anweisungen.

Modellantworten und Verbrauchswerte in diesen Nachweisen sind festgelegte
lokale Testantworten. Die Extraktionsqualität eines realen Modells ist damit
nicht bewertet. Frühere private Browsernachweise und die ursprünglichen
fehlgeschlagenen Prüfläufe bleiben erhalten. Im aktuellen Browserlauf gab es
genau eine Modellanfrage mit Rollenauftrag und gebundenen Dokumentdaten,
eine bestätigte Antwort und keine Dateiänderungen oder Aktionsfreigaben.
Eine nach der Planvorbereitung geänderte Originaldatei blieb erhalten;
Erkennung und Vorschläge verwendeten weiterhin die bestätigte Eingabekopie.

Der erste Main-Browserversuch scheiterte beim Dokumentprogrammstart in einem
überlangen internen Arbeitsverzeichnis. Die ursprünglichen Fehlversuche und
die erfolgreiche Wiederholung mit derselben Binärdatei und kurzem Laufzeitpfad
bleiben unverändert erhalten.

Die **Windows-Pfadvorprüfung** wurde zunächst als begrenzter Path14-Schritt im
Main-Quellstand qualifiziert. Vor Probe, Auftrag und Start prüft sie das tatsächlich
benötigte interne Arbeitsverzeichnis; verschachtelte LaTeX-Verzeichnisse
haben eine eigene Grenze. Nicht unterstützte Pfade wurden vor Annahme eines
Plans oder Laufs verständlich abgewiesen. In dieser damaligen Variante musste
der Betreiber gegebenenfalls einen kürzeren privaten Datenpfad wählen; die
automatische Ergänzung vom 02.11.2026 ist unten getrennt beschrieben.

Der historische gebundene Nachweis (development artifact omitted from public sources) bezieht sich auf
die Main-Epoche mit 5.075 Quelleingaben: 763 Oberflächentests, Typprüfung/Webbuild,
15 Windows-Executor-, 23 Toolbox-, eine Runtime-Vorabprüfung, zwei Prüfformular-
und sechs native Windows-Dokumentfälle sowie 37 ausgewählte Linuxfälle bestehen.
Je Plattform bleibt ein deklarierter Toolboxfall ausgelassen; bedingt
konfigurierte Office-/LaTeX-Fälle sind keine physische Formatabnahme. Der
neue Windows-Native6-Lauf besteht im ruhigen Fenster mit den ursprünglichen
Fristen. Die Browserprüfung weist den langen Datenpfad ohne Plan/Lauf/Modell
ab; im kurzen Pfad stellt ein modellfreier Textlauf nach hartem Dienstneustart
denselben Graphen und dieselbe Originalausgabe automatisch wieder her.

Dieser historische Schritt belegt die Vorabdiagnose für nicht unterstützte
interne Windows-Prozesspfade. Die automatische Anbindung langer Datenpfade (F71) blieb in diesem
damaligen Stand offen; die aktuelle Ergänzung ist unten beschrieben. Frühere Zeitüberschreitungen werden aufbewahrt; der neue grüne Lauf
beweist nicht deren alleinige Ursache. Der getrennte Runtime-Komponentennachweis
berücksichtigt die beiden Executor-Änderungen und besitzt eine eigene Quellenbindung.
Installierte App, Modellgüte und Parserisolation sind weiterhin nicht abgenommen.

## Ergänzung vom 02.11.2026: kurze private Windows-Arbeitsablage

Der Windows-Daemon kann jetzt einen langen ursprünglichen Datenpfad beibehalten:
Passt dessen physischer Dokumentpfad nicht in alle Prozess-, Office- und
LaTeX-Budgets, richtet er beim Start automatisch eine private Arbeitsablage unter
`<KnownFolder_Profile>/.localai-cowork-documents/work/<Locator>` ein.
Der Benutzerprofilordner kommt direkt von Windows; die Kennung gehört zum
ursprünglichen Toolbox-Ordner. Die Oberfläche wählt keinen Ersatzpfad.
Datenbank und ursprüngliche Benutzerdateien werden nicht verschoben.

Die gespeicherte Quellenbindung wird bei jedem Dienststart mit den ursprünglichen
Verzeichnisidentitäten abgeglichen. Das Kopieren einer Kennungsdatei oder der
Austausch eines gebundenen Ordners erlaubt keine neue Zuordnung. Dokumentrevision
und bestätigte Pläne binden die ausgewählte Ablage weiterhin. Quellenpfad,
Dateirechte, Inhaltsprüfsumme und bestätigte Eingabekopie behalten ihre Bedeutung;
die temporäre Arbeitsablage erteilt keine zusätzlichen Inhaltsrechte und übernimmt
keine Ergebnisse in die Originaldatei.

Die physische Vorabprüfung bleibt auch für die kurze Ablage aktiv. Ist sie weiterhin
zu lang, reicht eine kurze Alias-Schreibweise nicht aus; Einrichtung oder Start
werden abgewiesen. Die Verfügbarkeits- und Planvorprüfung behält die Diagnose
`data_path_too_long`; scheitert schon die private Einrichtung, wird kein
ungeeigneter Ersatz verwendet. Bei einer Serververbindung betrifft die private
Ablage den Windows-Benutzer des Daemons, nicht den Browserrechner.

Der neue modellfreie Main-Nachweis prüft fünf echte Dienststarts mit derselben
Quellenbindung und belegtem Ende der Originalprozesse. DOCX, XLSX und PPTX,
echte Office-Konvertierung und Formelneuberechnung, PDF, Bildbearbeitung,
native OCR mit erkanntem Beispielbetrag `119,00`, isoliertes LaTeX sowie
Browserbearbeitung und geprüfte Downloads bestehen. Vorbereitung und Bestätigung
überleben Neustarts; eine unterbrochene Dateiveröffentlichung wird über ihren
ursprünglichen Vorgang wiederhergestellt. Diese Dokumentwerkzeug-Probe nutzt
kein Modell und ersetzt nicht den oben getrennten Extraktions-/Prüfformularnachweis.

Die historischen Path14-Zahlen und ursprünglichen Fehlversuche bleiben unverändert
erhalten. Die neue Arbeitsablage ermöglicht den geprüften langen Datenpfad durch
kurze interne Arbeitsverzeichnisse; sie ist keine Aufhebung der Windows-Pfadgrenzen.
Paketprüfungen haben eigene Nachweise. Installierte App, Hardwarebreite, reale
Modellqualität und die vollständige Rechnungskette sind damit nicht abgenommen.
Weitere Einzelheiten stehen in [DOCUMENT-TOOLS.md](DOCUMENT-TOOLS.md).

Automatische Seitennachladung, Reparatur ungültiger Modellergebnisse, dauerhafte
Ergebnisübernahme, Tabellenbuchung, Ablage und Meldung im gemeinsamen
Rechnungsablauf bleiben Teil des
[vollständigen Automatisierungsausbaus](AUTOMATION-IMPLEMENTATION.md).
Eine installierte App und ein isolierter Parser sind durch diesen funktionalen
Host-OCR-Nachweis nicht abgenommen.
