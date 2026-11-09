# Vollständiger Automatisierungsausbau

Auftrag vom 29.10.2026: den gesamten vorgeschlagenen Verbesserungsplan umsetzen.
Basis: LocalAI Cowork V2, 2.0.0-alpha.7. **Gesamtstatus: in Umsetzung.**
Dieser Auftrag ist erst abgeschlossen, wenn alle unten aufgeführten Anforderungen
implementiert und im jeweils beanspruchten Umfang geprüft sind. Einzelne grüne
Tests, vorhandene Quellpfade oder ein erfolgreicher Build ersetzen diese Abnahme nicht.
Bestehende Änderungen und historische Nachweise bleiben erhalten.

Der bekannte erste Schema-101-Stand bleibt mit seiner ursprünglichen Prüfsumme
startfähig. Migration 102 übernimmt Ereignisse aus diesem und dem ergänzten
Schema-101-Stand in dieselbe verbindliche Tabellenstruktur. Sie verwendet die
eingefrorene Aufgabe aus dem ursprünglichen Eingangsbeleg, auch wenn der Trigger
inzwischen einer anderen Aufgabe zugeordnet wurde. Abweichende Belege oder
unbekannte Prüfsummen werden abgewiesen; die bisherigen Migrationsbelege werden
nicht umgeschrieben. SQLite, ein eigener PostgreSQL-Testbestand und der erhaltene
Walkthrough-Bestand sind dafür geprüft. Dies schließt die übrigen Anforderungen
dieses Automatisierungsauftrags nicht ab.

## Verbindlicher Umfang und Nachweise

| ID | Anforderung | Erforderlicher Abschlussnachweis | Stand |
|---|---|---|---|
| A01 | Dateiänderungen, Webhooks, E-Mail, Kalender, Formulare und Ablaufabschlüsse als Ereignisse; Filter, Entprellung, Dublettenerkennung | Echte Ereignisse je Adapter, Neustart, doppelte Zustellung, Widerruf, verpasste Ereignisse und angemessene Ressourcenlimits | Manueller/Webhook-Eingang und lokale Dateibeobachtung samt Filtern, Entprellung, Queue und UI implementiert; weitere Adapter und Gesamtabnahme offen |
| A02 | Typisierte Datenübergaben, Bedingungen, Verzweigungen, Listen, Zusammenführung, Warten und Unterabläufe | Editor → veröffentlichte Definition → echter Lauf für jeden Baustein, Typ-/Grenzfehler und Wiederaufnahme | Typisierte Eingaben, Feldbindungen, Ereigniszuordnung, native Bedingungen, Zweig-Gates, Auswahl/Sammlung und dauerhafte relative Wartezeiten implementiert; eingefrorene Unterabläufe und flache Listenverarbeitung im öffentlichen Produktpfad; ausdrückliche portable Arbeitsbereichsauswahl ergänzt; absolute Termine, Ereigniswarten und Gesamtabnahme offen |
| A03 | Geführte App-Verbindungen und fachliche Aktionen für APIs, E-Mail, Kalender und Dateispeicher | Anmeldung, Rechte, Erneuerung, Widerruf, echte Aktionen und verständlicher Status; keine Accountdaten im Export | Allgemeiner Connector vorhanden; fachliche Integrationen offen |
| A04 | Hintergrundbetrieb, Warteschlangen, Nachhol-/Überlappungsregeln und Wiederaufnahme | App schließen, Schlafmodus, Geräte-/Serverausfall, Neustart und nachweislich unveränderte bestätigte Wirkungen | Ereignisqueue, gespeicherter Dateibeobachtungsstand und Workerfreigabe bei dauerhaften Timerpausen implementiert; durchgängiger Betrieb offen |
| A05 | Dauerhafte begrenzte Ausführungsrechte nach Aktionen, Zielen, Empfängern, Zeit und Budget | Wirksame Grenzprüfungen unmittelbar bei Aktionen, Widerruf und Freigabe bei Überschreitung | Bestehende Aufgabenrechte wiederverwendet; ausdrückliche Auftragsarbeitsbereichsauswahl und fremde Ordnerprüfung vor Werkzeugzulassung ergänzt; dauerhafte Regeloberfläche/-verträge offen |
| A06 | Testereignisse, Beispieldaten, schrittweise Ausführung, Ein-/Ausgaben, geeignete Wiederholungen und Ergebnisklärung | Testlauf erzeugt keine externen Änderungen; Debugging und Fehlerbehebung über echte unterbrochene Läufe | Nebenwirkungsfreie Filter-/Eingabevorschau, Ereignisbeispiele, Verlauf und Dateibeobachtungsstatus implementiert; Ablaufdebugger offen |
| A07 | Dokumenteffekte in Workflows, OCR, Extraktion, Vorlagen, Tabellenänderungen und Vorschauen | Rechnungsreferenzablauf mit echten Dokumenten, OCR-Fehlern, Prüfung, Freigabe und Neustart ohne doppelte Datei | Native Dokumentaktionen, OCR, typisierte Excel-Buchungen und Prüfformulare sind Main (historische OCR-TABLE-VALIDATION.json/WORKFLOW-REVIEW-VALIDATION.json). Quellenansicht und Extraktionsstarter mit neuer ausdrücklicher Kontextregel ebenfalls Main:749 UI/751 frisch erzeugte Verträge/196 Domain; ausgewählte Windows-/Linux-OCR-/Extraktions-/Wiederanlauffälle und volle Speicher-Lib89/88 bestanden, jeweils6 deklarierte PostgreSQL-Fälle ignoriert. Die Kontextkorrektur gilt nur für neue veröffentlichte gewählte Versionen; alte Versionen behalten ihren Modus. Neuer gemeinsamer Main-Browsernachweis mit kurzem Windows-Laufzeitpfad bestanden: ausdrueckliche Version, echte OCR19, ein Modellaufruf mit zwei Nachrichten, native Korrektur/eine Antwort, harter Dienstneustart und automatische Originalanzeige (DOCUMENT-EXTRACTION-VALIDATION.json). Der erste fehlgeschlagene Langpfadversuch bleibt erhalten. Die anschließende begrenzte Windows-Pfadvorprüfung ist Main-qualifiziert (DOCUMENT-RUNTIME-PATH-VALIDATION.json, Epoche 5.075 Quellen): 763 UI mit Typprüfung/Webbuild, 15 Executor-/23 Toolbox-/1 Runtime-Vorabprüfungs-/2 Prüfformular-/6 native Windows-Dokumentfälle, 37 ausgewählte Linuxfälle sowie zwei tatsächliche Browserwege mit 0 Modellanfragen. Je Plattform bleibt ein deklarierter Toolboxfall ausgelassen. Das lange interne Arbeitsverzeichnis wird vor Plan/Lauf abgewiesen; der kurze modellfreie Textlauf stellt denselben Graphen nach hartem Neustart wieder her. Der neue Windows-Native6-Lauf verwendet unveränderte Fristen im ruhigen Fenster. Frühere RED-Nachweise bleiben erhalten; eine alleinige Ursache früherer Zeitüberschreitungen ist damit nicht bewiesen. Echte Langpfadunterstützung/F71 bleiben offen; die zwei Executor-Änderungen gehören zum getrennten Runtime-Komponentennachweis. Bedienung/Grenzen: DOCUMENT-EXTRACTION.md. Automatische Seitennachladung, Modellfehler-Reparatur, persistente Übernahme, Ablage/Meldung und volle Rechnungsreferenz bleiben offen; keine installierte oder gesamte Linux-App-Abnahme. |
| A08 | Bearbeitbare Abläufe aus natürlicher Sprache, gezielte Rückfragen und Test vor Aktivierung | Beschreibung → gültiger Entwurf → bearbeitbarer Ablauf → Test → ausdrückliche Aktivierung mit fehlenden Ressourcen | Offen |
| A09 | Wiederverwendbare Browser-/Desktopaktionen, beispielhafte Aufzeichnung, stabile Elementerkennung und manuelle Übernahme | Reale unterstützte Browser/Desktops, geänderte Seiten, Login-/Sitzungsfehler, Abbruch und Ergebnisprüfung | Browser-/Geräteunterbau vorhanden; neue Ablaufbausteine offen |
| A10 | Hybride Wissenssuche, Quellen, Aktualität, gezielte Aktualisierung und freigegebene Folgeereignisse | Relevanzkorpus, Herkunft bis zur Quelle, Rechte/Entzug und belegte Aktualisierung | Bestehende Wissensbasis vorhanden; Ausbau offen |
| A11 | Zentrale Freigaben mit Formularen, Vorschauen, Fristen, Zuständigkeiten, Hintergrundmeldungen und mobile Freigaben | Echte geschlossene Desktopansicht und erreichbares Telefon; Antwort-/Widerruf-/Fristkonflikte | Bestehende Freigaben vorhanden; vollständiger Ausbau offen |
| A12 | Betriebsübersicht, Besitzer/Übergabe, Verbrauch, Entwurf/Produktion, Versionsprüfung, Wiederherstellung und signierte Updates | Reale Teamrollen/Übergabe, Versionsfreigaben, Backup/Restore, aktuelle installierte signierte Pakete und Releasegates | Ereignisverlauf/Zuordnung in Umsetzung; Gesamtabnahme offen |

A07 baut auf dem bestehenden Dokument-/Medienunterbau aus
[DOCUMENT-MEDIA-IMPLEMENTATION-2026-10-23.md](DOCUMENT-MEDIA-IMPLEMENTATION-2026-10-23.md)
auf. Dessen offene Workflow-, OCR-, Formatqualitäts- und Gerätepfade werden im
Automatisierungsausbau weitergeführt.

## Lieferreihenfolge

1. Vorhandenen lokalen Ausführungspfad mit Einrichtung, Fähigkeiten, Modellen,
   Werkzeugen, Freigaben, Abbruch und Wiederaufnahme vollständig qualifizieren.
2. Rechnungsreferenz: neue Datei → Text/OCR → strukturierte Extraktion →
   Pflichtfelder/Unklarheiten → Nutzereingabe → Tabellenänderung → Ablage → Meldung.
3. Geführte Einrichtung, Ablaufeditor, Erstellung per Beschreibung und zehn
   vollständige Vorlagen; fachliche Verbindungen nach deren Anforderungen.
4. Unbeaufsichtigter Betrieb, begrenzte dauerhafte Rechte, mobile Rückfragen,
   Modell-/Ressourcenregeln und Betriebsübersicht; mehrtägige Läufe.
5. Teamrollen, Übergaben, Test-/Produktivversionen, Wiederherstellung und
   signierte Updates. Desktop, Web, Geräteagent und separater Enterprise-Server
   benötigen jeweils eigene durchgängige Nachweise.

### Verbindlicher Vorlagenkatalog

- T01 Rechnungen erfassen und ablegen.
- T02 Dokumente nach Inhalt sortieren und benennen.
- T03 E-Mails zusammenfassen und Antwortentwürfe vorbereiten.
- T04 Termine mit Projektunterlagen vorbereiten.
- T05 Wöchentliche Berichte aus Dateien und Tabellen erstellen.
- T06 Webseitenänderungen überwachen und relevante Änderungen melden.
- T07 Formulare prüfen und fehlende Angaben anfordern.
- T08 Besprechungsnotizen in Aufgaben überführen.
- T09 GitHub-Issues vorsortieren und Bearbeitungsvorschläge erstellen.
- T10 Neue Projektunterlagen auswerten und einen belegten Überblick erzeugen.

Jede Vorlage benötigt einen echten Ausführungspfad, geführte Verbindungen,
bearbeitbare Parameter, Beispieldaten, Fehlerfälle und Ergebnisprüfung. Ein Prompt
oder ein Katalogeintrag allein schließt keine Vorlage ab.

## Aktuelle Implementierung: dauerhafte Ereignisse

Neue Dateien: `crates/contracts/src/automation.rs`,
`crates/domain/src/automation.rs`, `crates/storage/src/automation.rs`,
`crates/runtime/src/automation.rs`, `services/daemon/src/automation_routes.rs`,
`packages/ui/src/AutomationPanel.tsx`, `packages/ui/src/automationApi.ts`.
Additive Migration 0101 ergänzt Auslöser, Ereignisse und Webhook-Grants.

- Auslöser binden eine gespeicherte Aufgabenrevision einschließlich ihrer
  Modell-/Workflow-, Werkzeug-, Ordner-, Budget- und Agentkonfiguration.
- Ereignisse erhalten getrennte ursprüngliche Eingangsbelege und aktuellen
  Zustand. Die gleiche Quellkennung mit anderen Daten ist ein Konflikt; exakte
  Wiederholungen liefern den unveränderten Eingangsbeleg.
- JSON-Objekte sind auf 32 KiB, Tiefe 32 und 4096 Werte begrenzt. Bis zu 32
  typisierte Filter unterstützen Existenz, Gleichheit, Text und Zahlenvergleiche.
- Filtervorschau schreibt keine Ereignisse und verwendet keinen Modellprovider.
- Die Ereignisqueue hält je Auslöser einen noch laufenden/ungeklärten Auftrag.
  Eine gebundene Originalanfrage wird vor der normalen Runtime-Zulassung gespeichert.
- Die tatsächliche Run-Erzeugung prüft Auslöser-/Aufgabenrevision und
  Verwerfung im selben Datenbankvorgang. Ein alter Worker erhält dadurch nach
  Pausierung, Aufgabenänderung oder ausdrücklichem Verwerfen keinen neuen Lauf.
- Ergebnisklärung liest ausschließlich originale Runbelege. Nach unbekanntem
  Start gibt es keinen automatischen Wiederholungsversuch. Ein Verwerfen sperrt
  einen späteren lokalen Start; es stoppt keinen bereits angenommenen Lauf.
- Webhooks verwenden einen eigenen Zugang für genau einen Auslöser, Ablaufdatum
  und Widerruf. Gespeichert werden nur Schlüsselhash und sekretfreie Metadaten.
  Browser-Origins und URL-Zugangsdaten werden nicht als Webhookautorität akzeptiert.
- Die Oberfläche unter „Aufgaben“ bietet Einrichtung, Filter, Testfelder,
  Ereignisversand, Verlauf, Verwerfen und Webhookzugang. Aufgabenänderungen
  erfordern ausdrückliches erneutes Binden. Anstehende sekretfreie Vorgänge
  behalten ihre ursprüngliche Kennung über Neuladen; Webhookschlüssel werden
  nicht im Browserjournal gespeichert.

## Dateiauslöser: gespeicherte lokale Beobachtung

Der nächste implementierte Schnitt erweitert dieselbe Ereignisqueue um die Quelle
`file`. Migration 0103 ergänzt einen versionierten Beobachtungsstand je Auslöser;
die bisherigen Ereignisse und unveränderlichen Eingangsbelege bleiben erhalten.
Der zusätzliche Status ist über
`GET /api/v2/projects/{project}/automation-triggers/{trigger}/file-watch` lesbar.
Dateiereignisse können nicht über den manuellen oder Webhook-Eingang gefälscht
werden.

- Die Einrichtung verwendet eine aktuelle, zur gebundenen Aufgabe gehörende
  native Projektordner-Freigabe. Die Beobachtung engt diese auf Lesezugriff ein.
  Ein relativer Unterpfad, Rekursion, Dateitypen, ausgeschlossene Ausgabepfade und
  die Ereignisse neu/geändert/entfernt sind konfigurierbar.
- Vorhandene Dateien werden standardmäßig beim Speichern als Ausgangsbestand
  übernommen. Erst nach diesem angenommenen Stand angelegte oder geänderte
  Dateien erzeugen Ereignisse. Die Verarbeitung des Ausgangsbestands verlangt
  eine ausdrückliche Auswahl. Ein fehlgeschlagener Ausgangsscan nimmt keine
  neue Konfiguration an. Eine unveränderte Konfiguration setzt bei Namens-,
  Aufgaben- oder Pausenänderungen den Beobachtungsstand nicht zurück.
- Inhalt und SHA-256 werden über den nativen Capability-Broker gelesen;
  Symlinks, Windows-Junctions, mehrfach hart verknüpfte Dateien und interne
  Cowork-Pfade erweitern den Zugriff nicht. Wird ein bisher regulärer Pfad
  unlesbar, weil er nun ein Link oder ein nicht unterstützter Eintrag ist,
  bleibt sein bisheriger Stand erhalten. Das ist kein Löschbeleg; der Status
  erklärt die Einschränkung.
- Je Prüfung gelten Dateianzahl-, Einzeldatei- und Gesamtbytegrenzen sowie
  Grenzen für Verzeichnistiefe und Einträge sowie ein zwischen I/O-Schritten
  geprüftes Zeitbudget. Ein blockierender Betriebssystemaufruf hat dadurch noch
  keine harte Abbruchgarantie. Ein abgebrochener oder
  unvollständiger Scan ersetzt keinen vollständigen Dateistand und erzeugt
  keine daraus abgeleiteten Löschereignisse.
- Inhalt muss über die eingestellte Entprellzeit unverändert beobachtet werden.
  Kandidat, ursprüngliche Kennung und Ausgangshash werden gespeichert. Eine
  Änderung während der Entprellung setzt sie zurück. Ein Neustart führt die
  gespeicherten Kandidaten fort; maximal 64 bestätigte Änderungen werden pro
  Beobachtungsabschluss in die Ereignisqueue übernommen.
- Beobachter verwenden zeitlich begrenzte Claims mit Revision und Kennung.
  Ein überholter oder abgelaufener Beobachter kann seinen Scan nicht mehr
  übernehmen. Ereignisannahme, Bestätigung der Dateibeobachtung und Cursor
  werden gemeinsam in einer Datenbanktransaktion gespeichert.
- Der Broker hält seine gemeinsame Ressourcensperre bis zur Übernahme des Scans.
  Eine bestätigte Rechteentziehung kann diese Übernahme nicht überholen. Nach
  Widerruf wird nicht weiter gelesen. Pausierung und Aufgabenänderungen sperren
  die Zulassung alter Beobachter ebenso wie alte Aufgabenstarts.
- Die Oberfläche zeigt lesbare Dateinamen und Änderungen, letzten vollständigen
  Stand, ausstehende stabile Inhalte und verständliche Fehlergründe. Kennungen
  und Rohdaten stehen im aufklappbaren Nachweisbereich.

Die Beobachtung vergleicht vollständige Stände in einem konfigurierbaren
Prüfabstand, mindestens fünf Sekunden. Sie erfasst keine Datei, die zwischen
zwei Prüfungen entsteht und wieder verschwindet. Auch nach einer Unterbrechung
werden nur Unterschiede zum letzten gespeicherten Stand erkannt. Ereignisse
enthalten Pfad, Ressourcenkennung und Inhaltsfingerprint; sie archivieren noch
keine unveränderliche Kopie der Quelldatei. Der spätere Dokumentablauf muss
diesen Fingerprint beim Lesen prüfen und geänderte Quellen behandeln. Diese
Bindung und der vollständige Rechnungsablauf bleiben Teil von A02/A07.

### Produktgrenzen dieses Zwischenstands

Ein laufender Daemon mit Worker bleibt Voraussetzung. Diese Änderung installiert
keinen Betriebssystemdienst und behauptet keine Ausführung bei ausgeschaltetem
Rechner. Die neuen Ereigniswege betreffen zunächst den persönlichen Ownerpfad;
Team-/Actor-/Offline-/Geräteadapter sind weiter Bestandteil von A01/A04/A12.
Der Rechnungsablauf und die zehn Vorlagen sind noch nicht abgeschlossen.

Maximal 100 Auslöser und 10.000 Ereignisse je Projekt verhindern unbegrenztes
Wachstum. Eine beleggebundene Archiv-/Aufbewahrungsverwaltung ist noch zu ergänzen;
historische Dublettenkennungen dürfen dabei nicht wieder ausführbar werden.
Ereignisse verleihen keine zusätzlichen Werkzeug- oder Dateirechte.

## Typisierte Workflow-Daten: erster A02-Schnitt

Format 2 ergänzt den bestehenden Rollen-/Schritt-DAG um definierte Eingabefelder,
Ergebnisfelder je Schritt und explizite Verbindungen. Der Editor bietet Text,
Zahl, sichere Ganzzahl, Wahr/Falsch, null, begrenzte Listen, Objekte und nullable
Werte. Alle Objektfelder sind verpflichtend; fehlende Werte werden nicht
stillschweigend in leeren Text oder null umgewandelt. Eine Liste als Datenwert
ist noch kein Baustein zur wiederholten Ausführung über ihre Elemente.

- Quellen sind ein fester Wert, ein Feld der Workflow-Eingabe oder ein Feld
  eines vorherigen Schritts. Ein Schrittbezug verlangt eine tatsächliche
  Abhängigkeit im wirksamen DAG; gleichnamige Felder verleihen keine Rechte.
  Bezüge auf nullable Unterfelder sind erst nach einer künftigen expliziten
  Behandlung des fehlenden Werts möglich. Listenpositionen müssen begrenzt sein
  und beim konkreten Lauf tatsächlich existieren.
- Die Veröffentlichung prüft Typen und Bezüge. Eingaben werden vor dem ersten
  Provider-Preflight geprüft, im angenommenen Lauf gebunden und in der
  gemeinsamen Startprüfung angezeigt. Modellantworten müssen das abgeleitete
  Ergebnisformat erfüllen. Ein falsches Ergebnis gibt keinen Folgeschritt frei;
  ein veränderter, erneut gehashter Datenrahmen wird beim Lesen abgewiesen.
- Jeder Schritt bekommt seine deklarierten Eingaben. Die neue Datenebene
  kopiert nicht automatisch alle rohen Vorgängerantworten oder alle ungebundenen
  Eingabefelder in jeden Modellkontext. Ausgewählte Rollen-/Projektkontexte und
  die ausdrücklich konfigurierte Aufgabe behalten ihre bisherige Bedeutung.
- Gespeicherte Chatkonfigurationen und Aufgabenrevisionen enthalten konkrete
  Werte. Ohne ausdrückliche Ereigniszuordnung bleiben diese unverändert. Der
  nachfolgend beschriebene Auslösermodus kann gewählte Eingabefelder mit
  typgeprüften Ereigniswerten ersetzen; andere Eingaben behalten ihre Aufgabenwerte.
- Definitionen, Requests und Bindungen ohne die neuen optionalen Felder behalten
  ihr ursprüngliches Format-1-JSON. Angenommene Läufe und Veröffentlichungsbelege
  werden nicht umgeschrieben. Es gibt keine neue Datenbankmigration und keine
  zweite ExecutionPlan-Zulassung. `input_snapshot_bound` bleibt false: gebundene
  Werte frieren keine Quelldatei ein.
- Grenzen gelten für Typverschachtelung, Feldanzahl, Listenlänge, Wertanzahl und
  Byteumfang. Format 2 erlaubt zunächst keine dynamische Rollendelegation und
  kein konkurrierendes Ergebnisformat auf Rollenebene. Vorhandene native
  Browserbausteine liefern Text und verwenden weiter ihr fest konfiguriertes
  Ziel; dynamische native Aktionseingaben sind noch zu ergänzen.

Bei der Qualifizierung wurde außerdem ein vorhandener Fehler in der nativen
Vorbereitung behoben: Annahme und feststehender Werkzeugvorschlag werden jetzt
in einer Graphrevision gespeichert. Vorher entstanden zwei Revisionen, die der
transaktionale Speichervergleich nicht annehmen konnte. Das private
`WorkflowProgress`-Format ist dafür 6; Formate 1–5 bleiben lesbar. Native
Vorbereitung verwendet denselben logischen Inhaltsdigest mit einer absoluten
Datengrenze, ohne ein ungenutztes Modell-Kontextfenster zu reservieren. Daraus
entsteht kein Modell-Sendepermit; echte Modellanfragen behalten ihre Prüfungen.

Die Rechnungsdaten im neuen Referenztest sind ausdrücklich synthetische
Modellantworten. Dieser Schnitt belegt weder Dateilesen/OCR noch eine fachlich
geprüfte Rechnung, Tabellenänderung oder den Abschluss von T01. Für Geldbeträge
braucht der vollständige Fachablauf eine ausdrücklich festgelegte genaue
Dezimal-/Minor-Unit-Darstellung; ein allgemeiner JSON-Zahltyp legt sie nicht fest.

## Ereignisfelder an Workflow-Eingaben binden

Der Auslösereditor bietet für typisierte Workflowaufgaben jetzt
**Workflow-Eingaben aus Ereignisfeldern**. Die Zuordnung ist opt-in und hat ein
eigenes Format 1. Zielfelder werden aus der unveränderlichen Workflowversion
ausgewählt. Dateiereignisse bieten bekannte Quellen wie Pfad, Inhaltsfingerprint,
Änderungsart und Größe an; andere Ereignisse verwenden Testfeldnamen oder
verschachtelte Quellpfade. Empfangene Ereignisse lassen sich als verschachtelte
Beispieldaten übernehmen. Die Vorschau zeigt Werte und gebundene Revisionen
auch bei pausiertem Auslöser, ohne Ereignisannahme, Datei-I/O oder Modellaufruf.

- Die Zuordnung enthält höchstens 64 explizite Ziel-/Quellpfade. Ziele müssen im
  Eingabetyp existieren; überlappende Eltern-/Kindziele werden abgewiesen.
  Ganze Objekte und Listen sind möglich. Eine einzelne Listenposition muss in
  der gespeicherten Aufgabenliste bereits existieren; es gibt kein implizites
  Auffüllen fehlender Einträge. Fehlende Quellen, falsche Typen und ungültige
  Gesamteingaben werden nicht stillschweigend ersetzt oder umgewandelt.
- Bei passendem, aktivem Eingang werden die abgeleiteten Werte zusammen mit
  Workflow-/Versionskennung, Definitionshash und Zuordnungshash im ursprünglichen
  Ereignisbeleg gespeichert. Fehlerhafte Eingaben werden dauerhaft als blockiert
  gespeichert, bevor ein Dispatchrequest oder Lauf entsteht. Solche Ereignisse
  sperren keine späteren gültigen Eingänge desselben Auslösers.
- Claim und tatsächliche Run-Zulassung prüfen die unveränderlichen Felder gegen
  den ursprünglichen Eingangsbeleg. Vor dem Claim wird außerdem die Projektion
  erneut gegen gebundene Aufgabe, Version und Zuordnung geprüft. Veränderte
  Auslöser-/Aufgabenrevisionen und gesperrte Workflowversionen erhalten keinen
  neuen Lauf. Verlorene Bestätigung und Wiederholung liefern den ursprünglichen
  Beleg mit denselben Werten zurück.
- Im neuen Modus wird die übrige rohe Ereignisnutzlast nicht zusätzlich an den
  Auftragsprompt angehängt. Nicht gebundene Eingaben gelangen über die Datenebene
  nicht in jeden Schritt; vorhandener ausdrücklich ausgewählter Chat-/Rollen-
  kontext gilt weiterhin. Ohne diesen Modus bleibt das bisherige Verhalten mit
  festen Aufgabenwerten und Ereignisdaten als Kontext kompatibel.
- Neue Felder sind optional und bei None ausgelassen. Die Domainprüfung sichert
  die unveränderten ursprünglichen JSON-Formen von Ereignis, Auslöser und Vorschau.
  Es gibt keine neue Datenbankmigration oder zweite Startzulassung.

Dies friert Datenwerte ein. Es erzeugt keine unveränderliche Dateikopie und
setzt `input_snapshot_bound` nicht auf true. Ein späterer Dokumentleseschritt
muss den erwarteten Inhaltsfingerprint weiterhin gegen die gelesenen Bytes
prüfen. Team-/Schedule-/Gerätezulassung und A05-Auftragsrechte werden hierdurch
nicht als abgeschlossen erklärt.

## Prüfstand

Neue gezielte Tests liegen unter `crates/{domain,storage,runtime}/tests/automation.rs`,
`services/daemon/tests/automation.rs` und `packages/ui/src/automationApi.test.ts`.

Tatsächlich bestanden am 29.10.2026 unter Windows für den ersten Ereignisschnitt:

| Prüfung | Ergebnis | Beleg unter `.local/automation-20261029/` |
|---|---|---|
| Domain-/Storage-/Runtime-/HTTP-Integration: `cargo test --locked -p cowork-domain -p cowork-storage -p cowork-runtime -p cowork-daemon --test automation` | 14/14 | `backend-tests.log` |
| Bestehende Aufgaben-/Zeitplanfälle: `cargo test --locked -p cowork-storage --test workspace_management --test native_schedules` | 11/11 | `storage-regressions.log` |
| Vollständige Frontendsuite: `npm test` | 437/437 in 84 Dateien | `frontend-tests.log` |
| `npm run typecheck` | Bestanden | `typecheck.log` |
| `npm run contracts:check` | 665 Rust-/Schema-/OpenAPI-/TypeScript-/Validatorverträge stimmen überein | `contract-check.log` |
| `npm run build` | Bestanden; bestehender Hinweis auf große JS-Chunks bleibt | `web-build.log` |
| Echter isolierter Chromium-Browser mit API-Fixtures | Einrichtung, Vorschau ohne Ereignis, verlorene Bestätigung → Reload → identische Kennung, genau ein Ereignis, Verwerfen, Webhookschlüssel ohne Browserpersistenz, Widerruf, 390-Pixel-Ansicht ohne horizontalen Überlauf und ohne JS-Seitenfehler | `ui-result.json`, `browser-tests.log`, `ui-desktop.png`, `ui-mobile.png` |

Die Runtimefälle verwenden einen Testprovider und prüfen tatsächliche lokale
Einzel-/Workflow-Runzulassung, unveränderte Werkzeuggrenzen, Überlappungssperre,
Pausierung nach Claim und Run-Readback nach verlorenem ACK. Sie belegen keine
Antwort eines echten LLM oder externe Werkzeugwirkung. Die Browserfälle verwenden
klar getrennte API-Fixtures; die HTTPfälle den tatsächlichen Axum-Router.
Die temporären eigenen Browser und der eigene UI-Server wurden beendet.
Vorhandene Benutzerinstallationen und Daemons wurden nicht umgestellt.

### Ergänzungsprüfung der Dateiauslöser

Die aktuellen Nachweise dieses Schnitts liegen unter `.local/file-watch-20261029/`.

| Prüfung | Ergebnis | Beleg |
|---|---|---|
| Windows: `cargo test --locked -p cowork-domain -p cowork-broker -p cowork-runtime -p cowork-daemon --test file_watch` | 20/20: Domain 7, Broker 5, Runtime 6, HTTP 2 | `backend-tests.log` |
| Ubuntu 24.04 unter WSL: Domain und Broker, `--test file_watch`, separater Targetpfad und `--offline` | 12/12 einschließlich echter symbolischer Links | `linux-tests.log` |
| Bestehende Ereigniswege: Domain-/Storage-/Runtime-/HTTP-Tests `--test automation` | 14/14 | `event-regressions.log` |
| Bestehende Aufgaben-/Zeitplanfälle | 11/11 | `task-regressions.log` |
| Vollständige aktuelle Frontendsuite: `npm test` | 445/445 in 86 Dateien | `frontend-tests.log` |
| `npm run typecheck`, `npm run contracts:check` und `npm run build` | Bestanden; 675 gemeinsame Verträge stimmen überein; bestehender Hinweis auf große JS-Chunks bleibt | `typecheck.log`, `contract-check.log`, `web-build.log` |
| Echtes Chromium mit tatsächlichem lokalem Axum, SQLite, FileBroker, Watcher, Queue und normaler Runtime | Neue Datei und Inhaltsänderung: je ein Ereignis und ein abgeschlossener Lauf; richtige aktuelle/vorherige Hashes; bestehende Datei übersprungen; externe Freigabe weiterhin nur lesbar; lesbare Ereignistitel; 390-Pixel-Ansicht ohne Überlauf und ohne JS-Seitenfehler | `browser-result.json`, `browser-tests.log`, `live-desktop.png`, `live-mobile.png`, `live-event.png` |

Die gezielten Tests verwenden reale isolierte Dateien und aktuelle native
Ordnerregistrierungen. Sie prüfen unter anderem partielle Schreibvorgänge,
Neustart mit gespeicherter Kandidatenkennung, Widerruf, überholte/abgelaufene
Beobachter, Pausierung nach Scan, Größenfehler ohne falsche Löschereignisse,
Hardlinks sowie Windows-Junctions. Der öffentliche HTTP-Eingang kann keine
Dateiereignisse einreichen und keine fremde Ordnerautorität erzeugen.

Der Browserfall ist mit
`services/daemon/examples/automation_file_watch_fixture.rs` wiederholbar.
Er startet eine eigene temporäre Datenbank und einen Loopback-Server. Die
Modellantworten stammen aus dem Testprovider; die Dateien tragen PDF-Namen mit
Testbytes. Das belegt Ereignisverarbeitung und lokale Run-Ausführung, keine
Dokumentextraktion, OCR oder Qualität echter Modellantworten. Der eigene
Testserver, UI-Server und Browser wurden beendet; das Backend hat kontrolliert
mit Exitcode 0 geschlossen. Benutzerprofile und laufende Benutzerinstallationen
wurden nicht umgestellt. Vorher-/Nachher-Hashes der erfassten Implementierungs-
und generierten Vertragsdateien stimmen für die Abschlussprüfung überein.

Die Linux-Prüfung deckt hier Domain und Broker ab. PostgreSQL wurde für den
neuen Dateibeobachtungspfad noch nicht geprüft; der oben dokumentierte
Migrationstest hat seinen eigenen Umfang. Eine gemeinsame Produkt-, Linux-
Runtime-, Telefon-, Installations- oder Releaseabnahme bleibt offen. A01–A12,
die fünf Lieferungen und T01–T10 bleiben als Gesamtauftrag offen; die geprüften
Ereigniswege schließen sie nicht allein ab.

### Ergänzungsprüfung der typisierten Datenübergaben

Nachweise dieses Schnitts liegen unter `.local/dataflow-20261029/`;
die Abschlussprüfungen laufen über den Datumswechsel zum 30.10.2026.

| Prüfung | Ergebnis | Beleg |
|---|---|---|
| Windows: Domain `workflow_dataflow` und bestehende `workflows` | 42/42, davon 9 neue Datenfluss-/Vorbereitungstests | `domain-tests.log` |
| Ubuntu 24.04 unter WSL, dieselben Domainfälle im separaten Offline-Target | 42/42 | `linux-domain-tests.log` |
| Windows/SQLite: Storage `workflows`, `native_schedules`, `workspace_management`, vier Testthreads | 42 reguläre Fälle bestanden; 7 PostgreSQL-/interne Prozess-Einstiege separat übersprungen | `storage-final-tests.log` |
| Windows: Runtime `workflow_dataflow`, `workflow_journal`, `workflow_product` sowie Modell-/Coordinatorfälle, vier Testthreads | 82 reguläre Fälle bestanden: 5 neue Datenflussfälle, 6 Journal-, 9 Produkt- und 62 Modellfälle; ein interner Prozess-Einstieg separat übersprungen | `runtime-final-tests.log`, `runtime-model-regressions.log` |
| Windows: tatsächlicher Axum-Router für Automation, FileWatch, Workflowjournal und Produktpfad, vier Testthreads | 19/19; neuer Start nur über Planbestätigung, Wiederholung mit Originalquittung und unveränderte Auth-/Origin-Grenzen | `http-final-tests.log` |
| Vollständige Frontendsuite, TypeScript und Web-Build | 464/464 in 89 Dateien; Typecheck und Build bestanden, bestehender Hinweis auf große JS-Chunks bleibt | `frontend-tests.log`, `typecheck.log`, `web-build.log` |
| Gemeinsamer Vertragscheck nach dem Generatorfenster | 684 Rust-/Schema-/OpenAPI-/TypeScript-/Validatorverträge stimmen überein | `contract-check.log` |
| Echter Chromium mit Produktionseditor, Eingabeformular, Planreview, tatsächlicher Owner-API und normaler Runtime; frisch gebautes, separat kopiertes Fixture-Binary | Veröffentlichung entspricht dem Editor; Typänderung sperrt ungültige Verbindung; Vorbereitung ohne Modellaufruf; nach Planbestätigung ein abgeschlossener Lauf und zwei Modellanfragen, Zahl bleibt Zahl und ungebundenes Testfeld bleibt aus dem Modellkontext; 390-Pixel-Ansicht ohne Überlauf/JS-Seitenfehler | `browser-result.json`, `browser-tests-final.log`, vier Editor-/Review-/Ergebnisbilder |

Die neuen Runtimefälle prüfen eingefrorene Eingaben, exakte zwei HTTP-Modellaufrufe,
fehlende/falsche Eingaben vor Providerkontakt, falsche Ergebnisse ohne
Folgeaufruf, gespeicherte Chat-/Aufgabenwerte nach Datenbankneustart und einen
manuellen Ereignisstart ohne implizites Überschreiben seiner Aufgabenwerte.
Ein manipulierter Datenrahmen wird auch nach Neuberechnung seines lokalen Hashes
abgewiesen. Die Modelle sind dabei synthetische lokale Fixtures.

Die Regressionen verwenden nun aktuelle Rechtekataloge und den gemeinsamen
ExecutionPlan-Start. Frische historische Schema-16/17-Fixtures ersetzen das
frühere Entfernen von Migrationsquittungen aus einer bereits aktuellen
Datenbank. Große aggregierte Testfutures liegen auf dem Heap; die Abnahme
verwendet keinen erhöhten `RUST_MIN_STACK`. Frühere fehlgeschlagene Diagnoseläufe
bleiben als solche erhalten. Dies lockert keine produktive Rechteprüfung.

Die Linux-Abnahme dieses Schnitts ist auf die Domäne begrenzt. Aktuelle
Storage-/Runtime-/HTTP-Nachweise gelten für Windows/SQLite; daraus folgt keine
neue PostgreSQL-, Linux-Runtime-, Team-, Telefon-, Installations- oder
Releaseabnahme. Die erfassten Quelldateien und fünf HTTP-Testfixtures erhalten
getrennte Vorher-/Nachher-Manifeste. Zwei parallele Änderungen sind als Delta
erhalten: Der vorbereitende Team-Preflight ist von den heutigen Owner-/Generator-
wurzeln nicht erreichbar und wird hier nicht abgenommen. Nach einer Änderung
an `DocumentWorkbench.tsx` wurden alle Frontendtests, TypeScript und der Web-Build
mit eigenem aktuellem Quellenmanifest erneut erfolgreich geprüft. Diese
Oberfläche ist nicht Teil des isolierten Datenfluss-Browserfalls.

`result.json` bindet Prüflogs, Bilder, Manifeste und die separat kopierte
Fixture-Binärdatei an ihre Hashes und enthält weiterhin ausdrücklich
`full_objective_complete=false`. Die eigene Browser-/UI-/Backend-Fixture ist
beendet; das Backend hat kontrolliert mit Exitcode 0 geschlossen. Es gab keine
Umstellung einer Benutzerinstallation, keinen Commit, Push oder Release.

### Ergänzungsprüfung der Ereigniszuordnung

Aktuelle Belege: `.local/event-inputs-20261030/`.

| Prüfung | Ergebnis | Beleg |
|---|---|---|
| Windows: Domain und Storage, `automation_inputs` plus bestehende `automation` | 20/20: Domain 3+5, Storage 6+6; nach Korrektur der alten Vorschauform alle sechs neuen Speicherfälle erneut bestanden | `domain-storage-tests.log`, `storage-final-tests.log` |
| Windows: Runtime `automation`, `file_watch`, `workflow_dataflow` | 16/16: 3 Ereignis-, 7 Datei- und 6 Datenflussfälle; manuelle und Webhookstarts mit zwei tatsächlichen HTTP-Modellaufrufen sowie Dateipfad/Hash bis zum angenommenen typisierten Workflow | `runtime-acceptance-tests.log` |
| Windows: HTTP-Automation und Dateibeobachtung | 6/6: vier Ereignis- und zwei Dateifälle, einschließlich Vorschau ohne Run und Zurückweisung vom Caller gelieferter Eingangssnapshots | `http-tests.log` |
| Ubuntu 24.04 unter WSL, Domainfälle im separaten Offline-Target | 8/8 | `linux-domain-acceptance.log` |
| Vollständiges Frontend, TypeScript und Web-Build | 467/467 in 90 Dateien; Typecheck und Build bestanden | `frontend-tests.log`, `typecheck.log`, `web-build.log` |
| Gemeinsamer Vertragscheck | 686 Rust-/Schema-/OpenAPI-/TypeScript-/Validatorverträge stimmen überein | `contracts-check.log` |
| Abschließende Ereignisregressionen nach der Vorschau-Kompatibilitätskorrektur | 16/16 über Domain, Storage, Runtime und HTTP; bestehende Auslöser geben weiter die ursprüngliche dreifeldrige Vorschau zurück | `event-final-regressions.log` |
| Echtes Chromium mit Produktionspanel, aktueller separat kopierter Fixture-Binärdatei, Owner-API, SQLite, Ereignistaktgeber, Queue und Runtime | Pausierte Vorschau ohne Run; falscher Typ vor Providerkontakt blockiert; Zuordnung gespeichert; verlorene Bestätigung → Reload → Originalbeleg; zwei Ereignisse (blockiert/gestartet), genau ein abgeschlossener Lauf und zwei Modellanfragen; feste Währung bleibt erhalten, ungebundene Ereignis-/Eingabefelder fehlen im Modellkontext; 390 Pixel ohne Überlauf und ohne JS-Seitenfehler | `browser-result.json`, `browser-compat-final-tests.log`, vier Bilder |

Die Modelle sind synthetische Loopback-Fixtures. Der Dateifall verwendet echte
isolierte Datei- und Broker-Metadaten; er belegt weder OCR noch Rechnungsextraktion
oder eine Tabellen-/Dateiwirkung. Die Linuxprüfung betrifft die Domäne, nicht den
vollständigen Linux-/PostgreSQL-/Team-/Telefonbetrieb. Frühere Diagnosen bleiben
erhalten: ein Testanbieter ohne strukturierte Ausgabe, ein fälschlich wieder
verwendeter Providerbezeichner und der zunächst fehlende Ereignistaktgeber in der
Browserfixture waren Prüfaufbaufehler. Produktive Rechteprüfungen wurden nicht
gelockert. Der echte Daemon startet seinen Ereignistaktgeber bereits unabhängig
von dieser Fixture.

Der Quellenvergleich umfasst 29 ausgewählte Implementierungs-, Test- und
Vertragsdateien. Der Übergang von der Vertrags-/Frontend-/Linux-/Browserprüfung
zur letzten Backend-/Browserprüfung erhält die zwei Dateien der nachfolgenden
Vorschau-Kompatibilitätskorrektur als Delta; ein zweiter Vorher-/Nachher-Vergleich
bindet den endgültigen Stand. Dies ist kein rückwirkendes Manifest der früheren
Rustläufe oder des gesamten parallelen Arbeitsbaums.
Der eigene Backend-/UI-/Browser-Testbestand wurde beendet. Es gab keine
Installation, Veröffentlichung, Commits, Pushes oder Vault-Schreiboperation.
Der Gesamtauftrag A01–A12, die fünf Lieferungen und T01–T10 bleiben offen.

## Nächste Implementierung

1. Begrenzte Listenverarbeitung, absolute Termine und Ereigniswarten im
   bestehenden Journalpfad ergänzen; verschachtelte externe Wirkungen weiter qualifizieren.
2. Journalisierte Dokumentaktionen/OCR in diesen Ablauf integrieren und die
   Rechnungsreferenz bis zum geprüften Ergebnis vervollständigen.
3. Weitere A01–A12-Adapter und die zehn fachlichen Vorlagen entlang der obigen
   Lieferreihenfolge mit aktuellen Nachweisen abschließen.

Die Ereigniszuordnung verwendet Auslöserrevision, Aufgabenrevision und
unveränderliche Workflowversion; Modellwahl, Werkzeuge, Ordnerfreigaben und
Budgets stammen weiterhin aus der gespeicherten Aufgabe und gemeinsamer
Runtime-Zulassung. Die folgenden Kontroll- und Dokumentbausteine müssen diesen
Originalbeleg und seine Eingangswerte bei Fortsetzung weiter verwenden.

Der bestehende Kern hat einen versionierten Rollen-/Schritt-DAG mit
unveränderlichen Veröffentlichungen, menschlichen Eingabe-Checkpoints,
Werkzeugjournal und Klärung unbekannter Wirkungen. A02 erweitert diesen Pfad.
Ereignisfelder und Schrittergebnisse werden dafür als typisierte Werte gebunden;
ein Dateieingang trägt den erwarteten Inhaltsfingerprint bis zum Dokumentlesen.
Zu prüfen sind insbesondere fehlende/falsche Felder vor einer Wirkung,
übersprungene Zweige beim Zusammenführen, begrenzte Listen mit stabilen
Elementkennungen, versionierte Unterabläufe und die Wiederaufnahme nach Warten.
Die bisherigen Format-1-Definitionen und Originalbelege dürfen dadurch nicht
anders interpretiert werden. Dokumentaktionen verwenden weiterhin ihre
bestehenden Freigaben und Wirkungsbelege.

Bestehende Releasegates (development artifact omitted from public sources) bleiben verbindlich, einschließlich
100 Sitzungen/40 Läufen, 24 Stunden Dauerbetrieb, Offlineabgleich, Tenanttrennung,
Backup/Restore, signierten Paketen und Barrierefreiheit. Erfolgsmaße sind
korrekte Ergebnisse, nötige Nutzereingriffe, Einrichtungsdauer,
Wiederherstellbarkeit und tatsächlich gemessene Kosten.

## Native Bedingungen und Zusammenführung: weiterer A02-Schnitt

Workflow-Definition Format 3 ergänzt native Kontrollbausteine ohne Rolle oder
Modellprovider. Format 1/2 lässt die neuen leeren Felder weiterhin aus; bestehende
JSON-Definitionen und deren Fingerprints behalten ihre Bedeutung. Format 3 nutzt
explizite Abhängigkeiten im Parallelmodus. Beim Ergänzen eines Bausteins übernimmt
der Editor die zuvor implizite Reihenfolge eines sequenziellen Entwurfs als Kanten.

- Bedingungen vergleichen benannte, typisierte Schritteingaben: Gleichheit,
  Ungleichheit, größer/kleiner, Text enthält, wahr und null. Alle/mindestens eine/
  Umkehrung erlauben begrenzte Kombinationen (höchstens 32 Terme, Tiefe 4).
  Es werden weder Code noch Modellprompts als Bedingungen ausgeführt.
- Gates binden einen nachweislich vorherigen Bedingungsbaustein und dessen
  gewünschten Wahrheitswert. Nicht gewählte Zweige und davon abhängige normale
  Schritte werden dauerhaft als übersprungen gespeichert. Sie erzeugen weder
  Modellturn noch Werkzeugfreigabe. Der Lauf zeigt einen lesbaren Übersprunggrund.
- Datenbausteine stellen aus den expliziten Eingaben ein typisiertes Objekt
  zusammen. Auswahl übernimmt genau ein vorhandenes Ergebnis; mehrere Ergebnisse
  führen zu einem bekannten Fehler. Ohne Ergebnis wird die Auswahl übersprungen.
  Sammlung übernimmt vorhandene Ergebnisse in deklarierter Quellenreihenfolge;
  übersprungene Quellen werden ausgelassen. Eine leere Sammlung ergibt `[]`.
- Merge-Quellen müssen vorherige Schritte mit passenden Typen sein. Fehlerhafte
  oder gesperrte Quellen werden durch die bestehende Fehlerstrategie behandelt;
  sie gelten nicht als übersprungene erfolgreiche Quellen.
- Native Arbeit verbraucht einen Ablaufschritt und keine Modelltoken. Globale
  und lokale Schritt-/Zeitgrenzen, Claim, Runlease, Widerruf und gespeicherte
  Revision gelten weiterhin. Kontrollentscheidung, Ergebnis und Abrechnung
  werden gemeinsam in einer Progress-Revision gespeichert. Native Entscheidungen
  geben keinen externen Dispatch-Permit aus.
- Private Progress-Version 7 speichert die ursprünglichen Kontroll-Eingaben einmal
  und spielt jede Entscheidung erneut deterministisch ab. Storage prüft diese
  Eingaben gegen die unveränderliche Runbindung. Normales Complete oder ein
  fingiertes Resume-Ergebnis kann keinen nativen Kontrollschritt abschließen.
  Eine unterbrochene reine Claim-Phase kann ausdrücklich fortgesetzt werden;
  bereits gespeicherte native Ergebnisse und Übersprungzustände bleiben erhalten.
- Der Editor ergänzt Bedingungen, Datenbausteine, Ergebnisquellen, Abhängigkeiten
  und wahr/falsch-Gates. Reine Kontrollabläufe können ohne Rollen und Modelle
  veröffentlicht und über den normalen Ausführungsplan gestartet werden.
  Beim Entfernen eines Knotens werden Ordnungskanten bereinigt; fehlende Daten-
  oder Bedingungsbezüge verlangen eine ausdrückliche Korrektur vor Veröffentlichung.

Der gebundene Prüfstand liegt unter `.local/control-flow-20261030/`; der abschließende
maschinenlesbare Bericht nennt die tatsächlich bestandenen Suiten und Grenzen.
Ein eigener Chromium-Lauf verwendet die Produktionseditoren, echte HTTP-
Veröffentlichung, Startprüfung, Queue, Runtime und Laufansicht. Eine im Editor
geänderte Grenze steuert zwei abgeschlossene Läufe; Modellanfragen und
Tokenverbrauch bleiben null. Die 390-Pixel-Ansicht zeigt keinen horizontalen
Überlauf. Die Prüfprozesse verwenden eigene Datenbanken und werden beendet.

Dieser Schnitt enthält noch keine dynamische Verarbeitung jedes Listeneintrags,
keine Warteschritte, Unterabläufe, Ablaufdebugger, OCR oder Rechnungswirkungen.
Er qualifiziert keine neuen Team-/Geräte-/Enterpriseadapter, PostgreSQL-Läufe,
installierten Pakete oder Releasegates. A02 und der Gesamtauftrag bleiben offen.

### Verifizierter Prüfstand dieses Kontrollschnitts

Der Bericht `.local/control-flow-20261030/result.json` bindet 56 ausgewählte
Quelldateien, Protokolle, eine frisch gebaute eigene Fixture-Binärkopie und
Desktop-/Mobilbilder. Bestanden: 52 Domainfälle unter Windows und dieselben 52
unter Linux, 31 SQLite-/Workflow-Storagefälle, 12 native/typisierte Runtimefälle
(einschließlich Ereigniszuordnung ohne Modellprovider), 62 bestehende Modell-
Runtimefälle im seriellen Testlauf sowie 476 UI-Tests. Typprüfung, 691 generierte
Verträge und Webbuild bestehen. Sieben Storagefälle bleiben als fünf dedizierte
PostgreSQL-Prüfungen und zwei durch Elternfälle aufgerufene Prozessfixtures
markiert; der Modelltest enthält ebenfalls ein solches Prozessfixture.

Der abschließende Browserlauf veröffentlicht eine im Produktionseditor geänderte
Bedingung und bestätigt zwei getrennte echte Starts. Beide gewählten Zweige,
Übersprunggründe, null Modellanfragen/Token und die 390-Pixel-Ansicht sind geprüft;
der eigene HTTP-/Queue-/Runtime-Prozess endet mit Status 0. Ein zusätzlicher
Runtimefall übernimmt ein Ereignisfeld in einen reinen nativen Workflow und
verhindert dessen erneuten Start bei doppelter Zustellung.

Numerische Gleichheit behandelt `25` und `25.0` auch in Objekten/Listen gleich;
Zahlen als Text und falsche Bool-/Nulltypen werden nicht umgewandelt. Kombinierte
Text-, Bool-, Null- und Zahlenbedingungen sind Teil der Domainprüfung.

Zwei zeitabhängige Modellfälle haben im parallelen Gesamtlauf ihre
Beobachtungsfrist überschritten. Das ursprüngliche Fehlerprotokoll bleibt
unverändert erhalten; dieselbe vollständige Suite besteht anschließend seriell
mit allen 62 Fällen. Produktfristen und Testassertionen wurden dafür nicht
geändert. Eine allgemeine Last-/Dauerbetriebsabnahme wird daraus nicht abgeleitet.

Gemischte Workflows prüfen beim Start weiterhin alle deklarierten Modellprofile.
Ein nicht gewählter Modellzweig erzeugt keinen Modellturn und keine Completion;
seine Verbindung wird dadurch noch nicht verzögert geprüft. Reine Kontrollabläufe
benötigen keinen Provider. Eine solche bedarfsweise Ressourcenprüfung gehört zum
noch offenen Ausbau der Ausführungs- und Modellregeln.

## Dauerhafte relative Wartezeiten: weiterer A02-/A04-Schnitt

Format-3-Kontrollbausteine unterstützen jetzt `wait` mit einer benannten,
ganzzahligen Sekunden-Eingabe. Sie lässt sich aus einem festen Wert, einer
Workfloweingabe oder einem vorherigen typisierten Ergebnis beziehen. Das feste
Ausgabeformat ist ein Objekt mit `wake_at` in UTC-Sekunden. Dauer, Typ und
ursprüngliches globales beziehungsweise lokales Zeitbudget begrenzen den Schritt;
bekannte ungültige unbedingte Werte werden vor einer Run- oder Modellzulassung
abgewiesen. Ein bedingter, nicht gewählter Schritt bleibt übersprungen.

- Ein Timer verbraucht einen Ablaufschritt und keine Modelltoken. Er erteilt
  keinen Werkzeug- oder Netzwerkpermit. Neue Timer speichern den tatsächlichen
  Millisekundenbeleg und runden den Fortsetzungstermin auf volle Sekunden auf,
  damit ein Sekundenwechsel die angeforderte Wartezeit nicht verkürzt.
- Private Progress-Version 9 ergänzt diese neuen Zeitbelege und die sichere
  ausdrückliche Wiederaufnahme eines bereits bestätigten ruhenden Timergraphs.
  Alte Entscheidungen ohne Zeitbeleg behalten ihre Serialisierung und Hashes.
  Ein gefälschtes Ergebnis oder eine normale Complete-Operation kann den
  Warteschritt nicht abschließen.
- Der Parent wechselt erst ohne aktive oder ungeklärte Wirkungen nach
  `waiting_for_timer` und gibt seine Workerlease frei. Migration 0109 speichert
  einen abgeleiteten Wake-Index mit Progress-Revision, Hash und Termin. Diese
  Projektion und die ursprüngliche Graphbindung werden beim Aufwecken erneut
  in derselben Parenttransaktion geprüft; der Index allein erteilt keine Rechte.
- Derselbe Lauf wird nach dem Termin wieder eingereiht. Ursprünglicher Start,
  Deadline, Eingaben und bestätigte Ergebnisse bleiben erhalten. Abbruch entfernt
  den Index; Widerruf und Ablauf verhindern Folgeschritte. Unsichere oder
  zurückspringende Ausführungszeit sperrt die Fortsetzung. Ein fehlerhafter
  abgeleiteter Index startet keine Arbeit und verhindert nicht die Zulassung
  unabhängiger normaler Aufträge; eine allgemeine Indexreparatur ist nicht Teil
  dieses Schnitts.
- Wird der Prozess zwischen Timercommit und Parent-Park unterbrochen, bleibt
  die konservative Unterbrechung erhalten. Der vorhandene Resume-Pfad kann einen
  vollständig ruhenden und geklärten Timer ausdrücklich erneut einreihen, ohne
  seinen Termin oder seine Versuche zu erneuern. Aktive oder unklare Wirkungen
  und eingereichte Ersatzresultate werden dadurch nicht zugelassen.
- Die Produktionsoberfläche ergänzt den Sekundeneditor, Feldbindung, laufenden
  Wartezustand und den lokalen Fortsetzungstermin. Die Aktivitätsübersicht zählt
  wartende Läufe weiterhin mit, ohne eine Nutzerfreigabe zu erfinden.

Ordnersperren bleiben während der Pause bestehen, weil dieser Timer keine neue
Workspace-Snapshot-Grenze einführt. Freie Worker können andere Aufträge mit
verfügbaren Ressourcen ausführen. Dauer und Wartezeit zählen weiter zum
ursprünglichen Zeitbudget. Absolute Termine, externe Ereigniskorrelation,
mehrtägige Läufe, Listeniteration, Unterabläufe und die übrigen Lieferphasen
bleiben offen. Der Timer ändert keine gemeinsame Actor-/Zielzulassung.

Die eigenen Prüfungen und Diagnosefenster liegen unter
`.local/wait-flow-20261030/`. Der zugehörige Bericht unterscheidet die aktuelle
Timerqualifikation von älteren Browser-/Vertragsprüfungen und der parallelen
gemeinsamen Plan-/Workspace-Anbindung. Es entsteht daraus keine vollständige
A02-/A04-, Team-/Geräte-, Native-GUI-, Hardware- oder Releaseabnahme.

### Verifizierter Timerprüfstand

Der Timerbericht (private development evidence; omitted from public sources) bindet die
ausgewählten Quellen, Protokolle, Testartefakte und Bilder. Bestanden sind je
59 Domainfälle auf Windows und Linux, 31 Workflow-Speicherfälle, 19 native und
typisierte Runtimefälle, 62 Modellfälle im seriellen Lauf und 485 UI-Tests.
Die Typprüfung und der Webbuild gehören zu diesem UI-Fenster. Die sieben
ignorierten Speicherfälle und das Modell-Prozessfixture behalten ihre bisherige
Kennzeichnung; sie sind keine zusätzlichen bestandenen Einzelprüfungen.

Ein separat ausgeführter PostgreSQL-Vertrag prüft vier echte Lebenszyklusläufe
auf Schema 110: Wiederöffnung und automatische Fortsetzung, Abbruch, Widerruf
sowie ausdrückliche Wiederaufnahme nach Timercommit vor Parent-Park. Der Audit
bestätigt zwei abgeschlossene, einen abgebrochenen und einen fehlgeschlagenen Lauf,
null Modellversuche und null verbleibende Wake-Einträge. Eigene Datenbank und
Testcluster sind nachweislich entfernt.

Die frische Chromium-Probe verwendet die Produktionseditoren, Veröffentlichung,
Owner-Planbestätigung und einen echten Worker auf einem eigenen Schema-112-Bestand.
Ein zweiter Auftrag endet vor dem Timertermin. Nach geschlossenem Tab und hartem
Neustart des eigenen Prozesses bleiben Originaltermin und Start erhalten; beide
Läufe enden ohne Modellanfrage oder Token. Die angeforderten 15 Sekunden ergeben
im Datenbankaudit einen mindestens 15 Sekunden entfernten Termin. Die mobile
Ansicht hat keinen horizontalen Überlauf. Der eigene Prozess endet mit Status 0;
71 ausgewählte Quellen sind im Browserfenster unverändert.

Die frühere Vertragsprüfung mit 696 Typen liegt vor den inzwischen ergänzten
gemeinsamen PlanV2-/RunWorkspace-Verträgen. Deren gekoppelte Generierung und
gemeinsame Prüfung bleiben ein getrennter offener Integrationsschritt. Der
Timerbericht verlängert diese ältere Vertragsabnahme nicht auf den neuen
gemeinsamen Umfang. Snapshotgebundene Timerläufe benötigen ebenfalls den
entsprechenden Adapter- und Wiederaufnahmenachweis. Der vollständige Auftrag
bleibt in Umsetzung.

## Öffentliche Unterabläufe und eingefrorene Vorlagenkopien

Das öffentliche Workflowformat 4 enthält `subflows` mit eingefrorenen
`WorkflowEmbeddedSubflow`-Bodydefinitionen, expliziten Argumenten, einer
Ergebnisquelle sowie eigenen Rechten und Budgets. Der Editor kopiert eine
ausgewählte veröffentlichte Projektversion unabhängig in den Entwurf. Spätere
Änderungen oder der Widerruf der ursprünglichen Vorlage ändern diese Kopie nicht;
der veröffentlichte Elternworkflow und die tatsächlichen Laufrechte bleiben
eigenständig widerrufbar. Die Quellenbindung umfasst den gesamten ursprünglichen
Baum und wird bei der Planprüfung rekonstruiert. Leere neue Felder bleiben in
bisherigen Definitionen und Plänen ausgelassen.

Die bestehende private `WorkflowComposition` dient dem Compiler intern weiter.
Argumente müssen exakt dem Eingabeobjekt des Bodys entsprechen; neue Kopien
starten mit ausdrücklich sichtbaren Konstanten, nicht mit automatisch übernommenen
Elterneingaben. Fremde Schrittbezüge außerhalb des Bodys werden abgewiesen.
Die öffentliche Bibliothek darf unbekannte Werkzeugnamen als unveränderliche
Daten behalten; die Produktvorbereitung prüft alle originalen Rechte rekursiv
und lehnt nicht unterstützte Produktwerkzeuge vor Modell- oder Werkzeugdispatch ab.

Der Compiler erzeugt daraus einen gemeinsamen Graphen mit stabilen Instanzkennungen.
Der private Plan behält die ursprüngliche Kompositionsquelle und deren Hash neben
der ausführbaren Definition. Eine erneute Kompilierung prüft Scopegrenzen und
Graphen vollständig; das bloße Neuberechnen eines manipulierten Planhashes reicht
nicht zur Zulassung. Alte Pläne lassen die neuen optionalen Felder aus und behalten
ihre ursprüngliche Serialisierung und Prüfsumme. Ein echter, aus dem früheren
eigenen Timer-Testbestand nur lesend entnommener Plan prüft diese Kompatibilität.

- Alle Bodyaktionen verwenden das gemeinsame Attempt-/Tokenledger. Lokale
  Rollen-/Schrittrechte werden mit Call-, Body-, Aufrufer- und Laufrechten
  geschnitten. Ein zweiter Modellkontext kann ein ausgeschöpftes Callbudget
  auch dann nicht umgehen, wenn der Elternlauf noch Token besitzt.
- Die Callfrist beginnt beim ersten Claim der privaten Argumentübergabe. Sie
  bleibt bei Unterbrechung und Wiederaufnahme erhalten. Lokale Parallelität wird
  bei jedem Claim geprüft; freie globale Slots vergrößern sie nicht.
- Private Argumentübergabe ist reine typisierte Verwaltung ohne Werkzeugpermit
  und ohne zusätzliche Attempts. Die Rückgabe zählt im Aufrufer und global;
  das eigene Bodybudget begrenzt die eigentlichen Bodyaktionen.
- Eine lokale Stopregel sperrt neue Bodyaktionen nach einem bekannten Fehler.
  Unabhängige Aufruferzweige dürfen unter `continue_independent` weiterarbeiten.
  Bereits unbekannte Modell-/Werkzeugwirkungen werden nicht erneut dispatcht.
- Native Timer können innerhalb eines Calls warten und nach Wiederaufnahme
  denselben Termin und dieselbe Callfrist behalten. Private Progress-Version 10
  kennzeichnet die neue Kompositionssemantik; bisherige Pläne verwenden weiterhin
  ihre ursprünglichen Versionen.

Der aktuelle Compiler begrenzt das gesamte Ergebnis weiterhin auf 64 Knoten und
32 Rollen sowie zusätzlich 16 Calls und vier Verschachtelungsebenen. Typisierte
sequenzielle Leaf-Bodys werden in explizite Kanten übersetzt; Quellen mit Calls
benötigen explizite parallele Graphabhängigkeiten. Hierarchische Komposition ist
noch nicht unterstützt. Diese privaten Grenzen werden nicht stillschweigend für
eine öffentliche Listenfunktion erhöht.

Der vorangegangene private Schnitt unter `.local/composition-flow-20261030/`
qualifizierte Compiler/Reducer mit elf neuen und59 bisherigen Domainfällen je
Plattform,23 nativen Runtimefällen und17 ausgewählten unveränderten Quellen.
Dieser historische Beleg bleibt unverändert; er behauptet keine damalige
öffentliche Oberfläche oder echte Providerdispatches. Die nachfolgende
Produktprüfung bindet die nun angeschlossene Veröffentlichung, rekursive
Modellauflösung, gemeinsame Startprüfung und Editor-/Laufansicht separat.

Listeniteration benötigt weiterhin dauerhafte Instanz-/Index-/Ergebniszustände
und begrenzte Gleichzeitigkeit innerhalb derselben Rechte und Budgets. Ein
eigener Korpus für verschachtelte Datei-/MCP-/Programmwirkungen, Widerruf und
ungewisse externe Wirkungen sowie echte Modell-, Geräte- und Teamabnahme bleibt
Teil des Gesamtauftrags. Die nachfolgende native Timer-/Arbeitskopieprüfung
qualifiziert nur ihren ausdrücklich beschriebenen Wiederaufnahmefall.

### Produktprüfung der öffentlichen Unterabläufe, 30.10.2026

Der aktuelle begrenzte Sammelnachweis (private development evidence; omitted from public sources) liegt unter
`.local/composition-product-20261030/`. Das finale Browserfenster erfasst1363
Quellen; vier parallele Änderungen im eigenständigen Runtime-Paketcrate werden
separat vermerkt und damit nicht qualifiziert. Die eigenen Produktquellen sind
in diesem Fenster unverändert. Er ersetzt weder die übrigen A02-Bausteine
noch eine reale Modell-, Desktop-, Team-, PostgreSQL- oder Releaseabnahme.

- Domain: 74 Fälle unter Windows und 74 unter Ubuntu-24.04, einschließlich
  bestehender Kontroll-/Daten-/Timergraphen und alter Planserialisierung. Der
  Linuxlauf bindet 112 unveränderte Eingaben und behält die fünf Testprogramme.
- Storage:31 reguläre Workflowfälle bestehen; sieben gesonderte PostgreSQL-/
  Prozessfälle bleiben im normalen Lauf ausdrücklich ignoriert.
- Runtime: 13 Datenfluss-, neun Produkt- und elf Timerfälle bestehen. Ein privater
  PostgreSQL-Timervertrag ist in diesem normalen Lauf ausdrücklich ignoriert;
  frühere PG-Nachweise behalten ihre eigenen Quellen und ihren Umfang.
- Der neue Modell-Unterablauf verwendet den tatsächlichen HTTP-/Streamingpfad
  einer synthetischen Loopback-Fixture: zwei Antworten, nur ausgewählte Daten,
  dieselbe bestätigte PlanV2-Arbeitskopie und kein weiterer Dispatch bei
  wiederholter Bestätigung. Falsche Ergebnisse und lokale Schrittgrenzen stoppen
  nach der ersten Anfrage. Das belegt keine Qualität eines realen Sprachmodells.
- Browser: Produktionseditor, typisierte Bindungen, Veröffentlichung und aktueller
  Arbeitskopie-Review laufen mit einem echten Worker. Nach verlorener Commitantwort
  liefert dieselbe Bestätigung genau einen Lauf. Eine Wartepause im Unterablauf
  überlebt einen harten Neustart mit unverändertem Start und Termin. Vier
  abgerechnete native Schritte, null Modellanfragen/Token, keine Seitenfehler und
  kein horizontaler Überlauf bei390px. Der eigene Fixtureprozess ist beendet.
- Oberfläche: 504 Fälle in98 Dateien, TypeScript und Webbuild bestehen. Die
  Darstellung gruppiert und nummeriert verschachtelt in Abhängigkeitsreihenfolge;
  Freigaben nennen den Aufruf statt einer erzeugten technischen Rollenkennung.
  715 gemeinsame Verträge sind generiert und auf Konsistenz geprüft.

Die reine Vorprüfung legt weiterhin keinen Orderworkspace an. Der Arbeitskopie-
Review bereitet den app-eigenen Workspace vor; die interne Scopeauflösung übernimmt
diesen bereits vorhandenen Default auch für Broker-Workflows. Vom Client gelieferte
verwaltete Ordnerkennungen erweitern keine nativen Projektordnerrechte. Ein
Wiederöffnen verwendet dieselbe ursprüngliche Brokeridentität.

Listeniteration, absolute Termine, Ereigniswarten und hierarchische Komposition
bleiben offen. Die nächste A02-Lieferung ist begrenzte Listeniteration; anschließend
folgen journalisierte Dokumenteffekte/OCR und die Rechnungsreferenz.

## Private Listenmaterialisierung: noch keine öffentliche Listenfunktion

Dieser Abschnitt dokumentiert den vorherigen Datenschritt. Der anschließende
Kernelabschnitt beschreibt den aktuellen privaten Integrationsstand.

Der nächste A02-Kernschritt in `domain/workflows/iteration.rs` friert eine
angenommene typisierte Liste ein. Originaldefinition, Knotenkennung und der
vom Elternlauf erwartete Eingangshash werden getrennt geprüft. Gleiche Werte
an verschiedenen Positionen erhalten eigene stabile Instanzkennungen. Eine
veränderte oder umgeordnete Liste kann den ursprünglichen Beleg nicht ersetzen.

Bodyargumente wählen ausdrücklich den Eintrag, ein Feld innerhalb des Eintrags,
den ursprünglichen Index, ausgewählte übergebene Felder oder Konstanten.
Gleichnamige Elternfelder werden nicht automatisch übernommen. Fehlende
Arraypositionen sind Datenfehler; es werden keine stillen Ersatzwerte erzeugt.
Gemeinsame Eingaben bleiben einmal im Manifest gespeichert. Das unterstützt
bis1000 Einträge innerhalb der bestehenden typisierten Werte-/Bytegrenzen,
ohne1000 Kopien des gemeinsamen Kontexts aufzubewahren.

Die Datensammlung verlangt genau die ursprünglichen Instanzen und korrekte
Ergebnistypen; fehlende, fremde oder falsche Ergebnisse werden nicht verworfen.
Die Ausgabe folgt dem Eingangsindex unabhängig von der Abschlussreihenfolge.
Ein leerer Eingang hat keine Instanzen und ein exakt leeres Datenergebnis.
Diese Werte sind keine IO- oder Ausführungsfreigabe.

Ein ausdrücklich nur im Test vorhandener Adapter führt wenige materialisierte
Einträge in einem vorhandenen Kompositionsgraphen aus. Dort werden gemeinsame
Schrittbudgets eingehalten und ein serialisierter Zwischencheckpoint setzt
ohne neue Versuche für abgeschlossene Einträge fort. Das ist noch kein
Produktpfad für eine während des Laufs gelieferte Modell-/Connectorliste.

Die nächste Integration muss die unveränderliche Planquelle von der daraus
abgeleiteten Ausführungsgraphansicht trennen. Die ursprüngliche Planbindung
darf beim Bekanntwerden neuer Elemente nicht durch einen fremden Planhash
ersetzt werden. Materialisierung, Instanzzustand und Originaleingang müssen
vor der ersten Bodywirkung in demselben Elternjournal übernommen werden.
Alle Instanzen verwenden dessen bestehende Rechte-, Attempt-, Token- und
Zeitgrenzen; separate Runs pro Eintrag sind kein Ersatz. Parallelitätsgrenzen
müssen aktive Einträge einschließlich wartender oder freigabepflichtiger Bodys
erfassen. Größere Graphen benötigen ausdrückliche Ressourcen- und Laufzeitprüfung;
die bisherigen64-/128-Knotengrenzen werden nicht stillschweigend erhöht.

Öffentliche Definition, dynamischer Reducer/Storage-Pfad, Editor, tatsächliche
Modell-/Werkzeugwirkungen, Abbruch/Widerruf und deren Prozess-/Browsernachweise
bleiben offen. Es gibt in diesem Kernschritt keine Migration, Vertragsgenerierung
oder Änderung an gemeinsamen Actor-/Controller-/RunWorkspace-/Ausführungsplantypen.

Der gebundene private Listenbericht (private development evidence; omitted from public sources)
bestätigt jeweils86 Domainfälle unter Windows und Ubuntu-24.04: zwölf neue
Materialisierungs-/Zuordnungs-/Ergebnisfälle plus74 bisherige Workflowfälle.
115 erfasste Eingaben bleiben im Linuxfenster und beim Abschlussvergleich
unverändert; Quellen- und Manifestdigest des festen Beispiels stimmen auf
beiden Plattformen überein. Materialisierung und Verifikation von1000 Einträgen
benötigten im aufgezeichneten Lauf482ms unter Windows und503ms unter Linux.
Dies misst die Datengrundlage, nicht1000 echte Modell-/Werkzeugausführungen.
Die sechs Testprogramme je Plattform sind separat erhalten. Formatprüfung
besteht. Strenges Clippy bleibt mit14 vorhandenen Bibliotheksbefunden außerhalb
des neuen Listenmoduls/-tests offen; der Originalbericht enthält die Stellen.


## Privater dynamischer Listenkern: öffentliche Produktintegration noch offen

Der nächste Kernschritt trennt den bestätigten `WorkflowPlan` von einer nicht
serialisierbaren `WorkflowExecutionGraph`-Ansicht. Der ursprüngliche Planhash,
die Profilbindungen und die Rechte bleiben eingefroren. Eine Liste kann erst aus
einem tatsächlich abgeschlossenen vorgelagerten Modellschritt bekannt werden.
Ihre Übernahme, ihr Manifest und alle neuen Pending-Knoten entstehen zusammen
mit genau einer nativen Journalentscheidung und einer Revision. Wiederholung
rekonstruiert die Instanzen aus den ursprünglichen angenommenen Daten; ein
nachträglich geänderter und neu gehashter Manifest-/Knotensatz wird abgewiesen.

Die private Quelle unterstützt bis16 flache Listenoperationen in einem
parallelen Elternworkflow. Jeder Body kann einen bestehenden typisierten
Format-2-/3-/4-Workflow einschließlich eingefrorener Unterabläufe enthalten.
Verkettete Listen sind geprüft; verschachtelte Listen und hierarchische Bodys
sind damit nicht eingeführt. Bodyrollen und Provider werden einmal je Vorlage
bestätigt und von den Instanzen geteilt. Es gibt keinen eigenen Run, Executor,
Arbeitsbereich oder Budgettopf pro Eintrag. Die übernommenen logischen
Arbeitsbereichsbindungen werden in den Agentenspezifikationen unverändert
beibehalten; daraus werden keine neuen nativen Verzeichniskennungen abgeleitet.

Einträge belegen ihre konfigurierte Listenparallelität, bis ihr Body beendet ist,
auch während einer Wartezeit oder ausstehender Arbeit/Freigabe. Bodyaktionen und
Rückgaben nutzen das bestehende Elternledger. Die Loop-Uhr beginnt bei der
ursprünglichen Übernahme; ein späterer Eintrag oder Neustart setzt diese Frist
nicht zurück. Native Argumentübergaben benötigen keinen Modellversuch. Die
abschließende Sammlung verlangt für jeden ursprünglichen Eintrag ein korrektes
Ergebnis; ein übersprungenes Ergebnis verkürzt die Liste nicht stillschweigend.
Leere Listen erzeugen keine Bodyinstanzen. Ergebnisse behalten die Reihenfolge
der ursprünglichen Indizes, auch bei doppelten Werten und anderer Abschlussfolge.

Die größeren Graphgrenzen gelten ausschließlich für diesen ausdrücklich
privaten Plantyp: höchstens4096 abgeleitete Knoten, zusätzlich eine konservative
32MiB-Grenze für die vervielfachten Graphvorlagen. Einzelne Argumentframes
müssen in die bestehende64KiB-Ausgabegrenze passen. Fehler dieser Grenzen oder
bekannter Warteeingaben verwerfen die gesamte Übernahme vor der ersten
Bodywirkung. Gemeinsame Eingaben werden aus einem Capture referenziert;
sie werden nicht als wachsende Literalduplikate in den Graph kopiert. Jeder
Workflowwrite prüft zentral die32MiB-Checkpointgrenze. Bestehende Pläne ohne
Iteration behalten ihre128-Knoten-Checkpointgrenze und ausgelassenen neuen
Felder. Der private Fortschritt verwendet bei Listenplänen Format11.

Die bisherigen Storage-/Runtime-Leser für native Timer, Modell-/Browser-
Vorbereitung, typisierte Datenframes und Scopeanzeige verwenden die abgeleitete
Ansicht. Der private Kernelbericht (private development evidence; omitted from public sources)
bindet die Prüfung an ihre konkreten Quellen und Testprogramme. Je104 Domainfälle
unter Windows und Ubuntu-24.04 enthalten18 neue Kernfälle und86 bestehende Fälle.
Drei zusätzliche private Storage-Adapterfälle prüfen ausdrücklich ausgewählte
Agentendaten, unveränderte Arbeitsbereichsreferenzen, gespeicherte Originalantworten
und deterministische Werkzeugvorschläge. Das sind serialisierte private
Checkpoints; sie ersetzen keinen neuen öffentlichen Listenstart mit tatsächlichen
HTTP-/Werkzeugwirkungen und Prozessneustart. Vorhandene Datenbank-/Runtime-
Regressionen qualifizieren weiterhin nur ihre konkret ausgeführten Produktpfade.

1000 Einträge wurden als3003-Knoten-Graph übernommen und wieder geöffnet:
6246ms unter Windows und6501ms unter Linux im gebundenen Debug-Durchlauf
mit parallelen Prüfungen. Das ist kein kontrollierter Leistungsbenchmark. Dabei
entstanden keine Modell-/Werkzeugversuche. Dies ist keine Abnahme eines
vollständigen1000-Element-Laufs. Das gemeinsame Beispiel behält auf beiden
Plattformen identische Quellen-/Manifestdigests. Neue Dateien bestehen ihre
Formatprüfung; strenges Clippy bleibt an den14 vorher dokumentierten
Bibliotheksbefunden offen.

Im abschließenden Quellenfenster bestehen zusätzlich34 Storageprüfungen und95
Runtimefälle, darunter alle62 regulären Modellfälle. Neun Fälle sind in den
betroffenen Storage-/Runtime-Suites regulär ignoriert; daraus entsteht kein
neuer PostgreSQL-Nachweis. Die Daemonprüfung und718 bestehende öffentliche
Verträge bestehen.1147 erfasste Quellen bleiben unverändert; die119 Domain-
Eingaben stimmen zwischen Windows und Linux überein. Die sieben Domainprogramme
je Plattform und die weiteren Windows-Testprogramme sind separat erhalten.

Die breite erste Modellrunde hatte drei Zeitüberschreitungen reproduziert.
Statische Definitionen, Native-Klassifikation und Scopeanzeige verwenden deshalb
die bereits eingefrorenen Metadaten; eine reine Abfrage spielt nicht für jeden
gespeicherten Turn das komplette Journal erneut ab. Neue Zustandsänderungen und
IO-Zulassungen behalten ihre vollständigen Prüfungen. Die unveränderte bestehende
Testdatei und ihre ursprünglichen Zeitgrenzen bestehen anschließend vollständig.
Die ursprünglichen Fehlerberichte bleiben neben dem neuen Quellenfenster erhalten.

Die öffentliche Workflowdefinition, Bibliotheks-/Preflight-/PlanV2-Anbindung,
Editorbedienung und echte Listenläufe mit Abbruch, Widerruf, verlorenen Antworten
und Prozess-/Browserneustart folgen als nächster notwendiger Produktschnitt.
Der Kernel führt keine Migration, neue öffentliche Verträge oder
Startautorität oder Änderung an Actor-/Controller-/RunWorkspace-/PlanV2-Typen
ein. Listenfunktion, A02 und Gesamtauftrag bleiben offen.

## Öffentliche Listenverarbeitung: Definition, Plan, Editor und echte Ausführung

Stand30.10.2026: Der öffentliche Format5-Pfad ergänzt den privaten Listenkern.
Der gebundene Produktnachweis (private development evidence; omitted from public sources)
qualifiziert diesen Schnitt; A02, A01–A12, T01–T10, die fünf Lieferungen und
Releasegates bleiben insgesamt offen. Die vorherigen privaten Berichte behalten
ihren ursprünglichen Umfang und werden nicht auf neue Quellen umgedeutet.

`WorkflowEmbeddedIteration` friert Vorlage, Rückgabefeld, Argumente, Rechte,
gemeinsame Grenzen und aktive Eintragszahl in der veröffentlichten Definition ein.
Der Compiler leitet den Capturetyp aus den ausdrücklich deklarierten Bindungen ab;
eine zweite widersprüchliche Eingabeschemaquelle wird nicht akzeptiert. Der
Originaldefinitionshash bleibt die Quellenbindung im bestätigten Plan. Die
abgeleiteten Instanzen verändern dessen gespeicherten Inhalt nicht. Root-
Unterabläufe und Listen können denselben Elternlauf und sein Ledger verwenden.

Es gibt keine neue Migration und keinen getrennten Startpfad. Actor-, Controller-,
RunWorkspace- und PlanV2-Strukturen wurden durch diesen Listenschnitt nicht
verändert. Im gemeinsam abgestimmten Generatorfenster entstanden 722 Typen,
einschließlich der separat entwickelten Importverträge; der abschließende
Rust-/OpenAPI-/SDK-/Validatorcheck besteht. Frühere Quellenformate lassen die neuen
Felder aus. Die tatsächliche ältere Timer-Planfixture behält ihren gespeicherten
Plan-SHA `843bb38c743928c7b5d24804bd53251114620784b7955fd275495cf95ef0a800`.

Die Produktprüfung untersucht ursprüngliche Rechte auch vor einer späteren
Schnittmengenbildung. Eine unbekannte deklarierte Capability wird nicht durch
Wegschneiden still aktiviert. Agentenspezifikationen übernehmen den bestätigten
Eingabearbeitsbereich unverändert; Modellkontexte enthalten ausschließlich die
expliziten Item-/Index-/gemeinsamen oder konstanten Argumente. Einträge werden in
öffentlichen Plänen in Indexreihenfolge zugelassen; parallele Abschlüsse bleiben
erlaubt. Ältere private Iterationsjournale behalten ihre historische Semantik.

Der neue Editor kopiert veröffentlichte Versionen, zeigt eingefrorene Inhalte,
Argumente und gemeinsame Grenzen und prüft Typen sowie Abhängigkeiten. Die
Format5-Wiederherstellung erhält auch Entwurfsdaten. Tatsächliche Browserbedienung
hat eindeutige zugängliche Bezeichnungen für Auswahlfelder und die Sortierung der
Eintragsgruppen nach dem Originalmanifest nachgezogen. Das sind keine neuen
Ausführungsrechte.

Geprüft sind je 111 Domainfälle unter Windows und Ubuntu-24.04 mit 123 identischen
Domaineingaben, 34 Windows-Storage- und 99 Windows-Runtimefälle, 546 Frontendfälle in 106
Dateien, TypeScript, Webbuild, fünf ausgewählte Rust-Formatprüfungen und 722 Verträge.
Die acht Domainprogramme je Plattform und die Windows-Storage-/Runtimeprogramme
sind erhalten.1535 erfasste Produktquellen bleiben im abschließenden Fenster
unverändert. Vorherige Fenster und Browserfehler bleiben gesondert erhalten.
Strenges Domain-Clippy ist nicht grün: 14 vorherige Befunde plus ein zusätzlicher
aktueller Environment-Befund des parallel entwickelten Runtime-Pakets; keine neue
Iterationsdatei ist betroffen. Sieben Storage- und zwei Runtimefälle sind regulär
ignoriert, daraus entsteht kein neuer PostgreSQL-Nachweis.

Vier neue Runtimefälle prüfen tatsächliche lokale HTTP-Modellanfragen und native
Ausführung: genau drei korrekt gebundene Modellanfragen bei maximal zwei aktiven
Sendern; Abbruch vor dem dritten Eintrag nach falsch typisiertem zweiten Ergebnis;
kein erneuter Versand nach unbekannter Antwort und Store-Reopen; Wiederaufnahme
der seriellen nativen Timerliste mit Originalmanifest, Plan, Lauf- und Loopstart.
Es sind deterministische HTTP-Fixtures und keine Abnahme der Qualität realer Modelle.

Der tatsächliche Browserfall verwendet den Produktionseditor, das gebaute
Frontend, die gemeinsame Planprüfung, einen echten Worker und eine eigene
Daemon-/SQLite-/Brokerinstanz. Er speichert und lädt den Listenentwurf, veröffentlicht
eine unabhängige Kopie, verwirft ungültige Eingabepfade/Ergebnistypen/Eintragsgrenzen
und lehnt unbekannte Rechte bei Preflight ab. Nach verlorener Commitantwort wird
dieselbe Bestätigung auf genau einen Lauf abgeglichen. Ein harter eigener
Daemonstopp während des ersten Timers und der Neustart am selben Endpunkt erhalten
Plan, Manifest, Lauf-/Loopstart und ursprünglichen Termin. Die zwei Einträge
liefern ihre geordneten Ergebnisse mit insgesamt 8 Schritten, 0 Modellversuchen und 0
Token; der unabhängige Datenbankaudit findet 1 Lauf, 11 Knoten und 0 verbleibende
Timerwakes.390px-Ansicht ohne horizontalen Überlauf und ohne Seitenfehler. Die
abschließende eigene Instanz endet regulär mit Code 0. Native Desktopoberfläche und
Barrierefreiheitsabnahme sind damit nicht qualifiziert.

Die frühen Browserdurchläufe enthalten getrennt einen Optimizer-Reload, die
Auswahlbeschriftung, eine fehlerhafte Statusantwortannahme des Testaufbaus und
unvollständige Timerbeobachtungen unter paralleler Bauarbeit beziehungsweise
aufwendiger Graphabfrage. Der letzte Nachweis läuft nach Ende der eigenen
Build-/Testprozesse, beobachtet den Abschluss über die Statusroute und behält die
ursprünglichen Zeitgrenzen bei. Daraus wird keine Last-/Leistungsabnahme abgeleitet.

Weitere Grenzen: flache Rootlisten, keine verschachtelten Listen oder
hierarchischen Bodys; statische Vorlagen-/Rollenlimits bleiben erhalten. Die
1000-Eintrag-Capture-/Reopen-Prüfung ist weiterhin kein vollständiger1000-Eintrag-
IO-Lauf. Absolute Termine, Ereigniswarten sowie der vollständige Rechnungs-/OCR-Ablauf bleiben
notwendige Anschlüsse des Gesamtauftrags. Die anschließend ergänzte ausdrückliche
Arbeitsbereichsauswahl wird im folgenden eigenen Quellfenster beschrieben. Auftragsgebundene alte
`workspace:<order>`-Rechte werden nicht automatisch auf andere Chats umgeschrieben.


## Portable Arbeitsbereichsauswahl und begrenzter Modell-Replay

`WorkflowRights.current_workspace` ist eine ausdrückliche Quellenauswahl. Der
Defaultwert false wird ausgelassen; frühere Definitionen und gespeicherte Pläne
behalten ihre Serialisierung. Bei einer positiven Auswahl werden die vorhandenen
Root-/Rollen-/Schritt-/Aufruf-/Listenrechte vor der normalen Kompilation auf den
verwalteten Ordner des aktuellen Auftrags bezogen. Alte konkrete Ordnerkennungen,
Prompts und Eingabewerte werden nicht umgeschrieben. Schnittmengen mit den
übergeordneten Rechten und sämtliche bestehenden Budgets gelten weiter.

Der private Workflowplan erhält die ursprüngliche unveränderte Quelldefinition
und eine logische Bindung aus Ordner-, Projekt- und Auftragskennung. Diese Bindung
ist keine NativeResource-ID und keine Dateiautorität. Der unveränderte umgebende
RunSpec/PlanV2 bindet weiterhin den tatsächlichen Ressourcengrant, dessen Revision
und die ausgewählte Eingabekopie. Das ist notwendig, weil die vorhandene
Copy-Projektion beim Start einen eigenen physischen Grant erzeugt. Die
Agentenspezifikation übernimmt die tatsächlichen Parent-Grants und
`input_workspace` unverändert. Beim Laden wird die private logische Bindung gegen
den tatsächlichen Parent geprüft; ein selbst neu gehashter fremder Scope reicht
nicht aus.

Die Auflösung verlangt genau einen bereits zugelassenen verwalteten Ordner dieses
Auftrags im richtigen Projekt. Fehlender oder mehrdeutiger Arbeitsbereich wird vor
Providerprobe und Runerzeugung abgewiesen. Dateivorschläge mit fremder
`folder_id` werden vor einer dauerhaften nativen Werkzeugzulassung und damit vor
einem Toolversuch abgewiesen. Der Broker prüft zusätzlich die tatsächliche Kopie,
Revision, Livefreigabe und den Pfad unmittelbar beim Zugriff. Diese Ergänzung
erteilt keine dauerhaften unbeaufsichtigten Aktionsrechte nach A05.

Der Editor bietet die Auswahl auf jeder relevanten Rechteebene ausdrücklich an,
erhält sie beim Vorlagenkopieren und Einengen vorhandener Agenteneinstellungen
und zeigt sie in der Zusammenfassung. Abhängige Auswahlen sind ohne übergeordnetes
Recht gesperrt; vorhandene ungültige Auswahlen lassen sich entfernen. Entwurf,
Wiederherstellung, Veröffentlichung und die bestehende bestätigte Ausführung
verwenden dieselben Originaldefinitionen. Bedienung steht in
[WORKFLOW-PRODUCT.md](WORKFLOW-PRODUCT.md).

Bei der neuen16-Modell-Listenprüfung wurde eine rekursive Gesamtwiederholung im
Complete-Reducer gefunden. Die Entscheidung verwendet jetzt den bereits
rekonstruierten geprüften Graphen statt über den öffentlichen Klassifikator das
Journal erneut zu replayen. Typ-, Claim-, Reihenfolge-, Zeit-, Versuchs- und
Tokenprüfungen bleiben erhalten. Der neue Fall führt16 Modellabschlüsse,
34 Versuche und32 Token im ursprünglichen Elternledger bis zum geordneten Ergebnis
und validierten Reopen. Ein erhaltener Vorher-Binary überschreitet90 Sekunden;
separat erhaltene korrigierte Debugprogramme schließen in etwa22–28 Sekunden ab.
Dies sind lokale Debugbeobachtungen, kein kontrollierter Leistungsbenchmark und
keine vollständige1000-Eintrag-IO-Abnahme.

Die breite Windowsregression deckte außerdem einen reproduzierbaren Stacküberlauf
im unveränderten parallelen Modelltest auf. Der verschachtelte Providerabschluss
wird nun im Runtimeadapter auf dem Heap gehalten; derselbe Test besteht ohne neue
Stackgröße, geänderten Test oder gelockerte Frist. Originalfehler und
Einzelreproduktion bleiben erhalten. Die15 angefragten Domain-Bibliotheks-Lints
wurden ohne neue Allow-Unterdrückung geschlossen. Drei lange Rust-Reduceraufrufe
verwenden benannte Argumentstrukturen; die gespeicherten Decision-/JSON-Felder
bleiben unverändert. Dieser Nachzug ist ein separates neues Quellenfenster nach
dem ersten fehlgeschlagenen Windowslauf.


### Gebundener Abschluss des Ressourcenanschlusses, 30.10.2026

Nachweis: resources-product-20261030/result.json (private development evidence; omitted from public sources).
Der persönliche Owner-/Brokerpfad ist im beschriebenen Umfang bestanden. Je153
Domainfälle unter Windows und Ubuntu-24.04,34 Storagefälle und126 Windows-Runtimefälle
bestehen; strenges Domain-Bibliotheks-Clippy ist grün. Sieben Storage- und vier
Runtimefälle bleiben regulär ignoriert. Es entsteht dadurch kein neuer
PostgreSQL-Nachweis.724 Rust-/OpenAPI-/SDK-/Validatorverträge sind konsistent.

Während des breiten Fensters wurden ausschließlich die zwei Brokerdateien
`workspace_transfer.rs` und `workspace_receive.rs` parallel geändert. Der breite
Quellenvergleich bleibt deshalb ausdrücklich false, obwohl sämtliche
Prüfkommandos erfolgreich sind. Auf dem neuen Brokerstand bestehen danach neun
Windows-Transferfälle und27 betroffene Snapshot-/Ressourcenfälle sowie der
Fixturebuild in einem eigenen unveränderten1164-Input-Fenster. Die unveränderten
128 Domaininputs stimmen plattformübergreifend überein. Die beiden konkreten
Brokerdeltas samt Vorher-/Nachherhashes bleiben im Bericht sichtbar.

Die tatsächliche IO-Prüfung verwendet dieselbe veröffentlichte Vorlage in zwei
Aufträgen: acht echte lokale HTTP-Modellanfragen, vier bestätigte Dateilesungen
jeweils aus der ausgewählten Eingabekopie, trotz anschließender Änderung der
Originaldatei. Die physischen Copy-Grants unterscheiden sich von der Originalquelle;
der Workflowcompiler speichert nur die logische Auswahl. Fehlender Workspace,
fremder Ordner vor Toolzulassung und gefälschter privater Scope werden abgewiesen.
Diese deterministischen HTTP-Fixtures prüfen den Transport und die Bindung,
keine Qualität eines echten LLM.

Nach visueller390px-Prüfung wurde die vorhandene Checkboxklasse ausdrücklich auf
Zeilenrichtung gesetzt. Der einzige anschließende Produktdelta ist diese
CSS-Eigenschaft. Danach bestehen erneut565 Oberflächentests in109 Dateien,
TypeScript, Web- und Browserbuild sowie die vollständige Browserbedienung.
Das finale Frontend-/Browserfenster bleibt auf1554 Produktinputs unverändert;
43 eigene Testprogramme und der separat kopierte Daemonbinary sind erhalten und
hashgeprüft. Vorherige Fehler, der erste Browserabschluss und die jeweiligen
Quellenfenster bleiben getrennt erhalten.

Der Browser speichert und lädt die positive Arbeitsbereichsauswahl zusammen mit
der eingefrorenen Listenquelle, veröffentlicht sie und verwendet den bestätigten
Plan mit Arbeitskopie. Eine verlorene Commitantwort wird über die identische
Bestätigung auf genau einen Lauf abgeglichen. Der harte Stopp des eigenen
Daemonprozesses während des ersten Timers und sein Neustart am Originalendpunkt
bewahren Plan, Quelldefinition, logische Bindung, Manifest, Lauf-/Loopstart und
ursprünglichen Termin. Abschluss mit8 Schritten,0 Modellversuchen und0 Token;
read-only Datenbankaudit: Schema112,1 Lauf,11 Knoten,0 Agents und0 Restwakes.
Editor und Ergebnisansicht zeigen bei390px keinen horizontalen Überlauf; keine
Seitenfehler. Die finale eigene Instanz endet regulär mit Code0.

Keine neue Migration, keine neue Dispatchautorität und keine Änderung der
gemeinsamen Actor-/Controller-/RunWorkspace-/PlanV2-Strukturen durch diesen
Ressourcenschnitt. Clippy gilt ausdrücklich für die Domainbibliothek. Native GUI,
Barrierefreiheit, Geräte/Android, Enterprise und Release sind nicht durch diesen
Beleg abgenommen. A01–A12, T01–T10 und die fünf Lieferungen bleiben insgesamt offen.
Der nächste fachliche Anschluss sind journalisierte Dokumentaktionen und die
vollständige Rechnungs-/OCR-Referenz; absolute Termine und Ereigniswarten bleiben
zusätzlich Teil von A02/A04.


## Journalisierte Dokumentaktionen in bestätigten Workflowkopien

Stand30.10.2026: `document_read`, `document_write` und `image_edit` verwenden in
Modellschritten dieselbe lokale Engine wie Chat und Dokumentwerkstatt. Dieser
Schnitt erweitert A07; er schließt A07 und den Rechnungsablauf T01 noch nicht ab.

Dokumentprogramme haben ein eigenes ausdrücklich gewähltes Ausführungsziel.
Der prozessfreie Dateibroker bleibt unverändert. Die Auswahl erfolgt vor der
Planvorbereitung; die anschließende bestätigte Planprüfung zeigt Betriebssystem-
und Netzwerkrechte des Benutzerkontos. Es wird keine Prozessisolation behauptet.
Allgemeine Programme, MCP und zusätzliche Umgebungen gehören nicht zu diesem
Dokumentziel. Vorbereiten, verlorene Bestätigungsantwort und Neuladen behalten
denselben ursprünglichen Ziel-/Eingabe-/Planbeleg. Ein Wechsel der gebundenen
Engine vor Commit oder Dokumentdispatch wird abgewiesen. Die Revision bindet
Adapter und ausgewählte Programmpfade/-binärdateien sowie den TeX-Cachemarker;
sie ist keine vollständige Abbildung aller Python-Wheeldateien.

Die Workflowdefinition muss die Dokumentwerkzeuge ausdrücklich erlauben.
Übergeordnete, rollen- und schrittspezifische Rechte werden weiter geschnitten.
Fremde Eingabe-/Zielordner werden vor Werkzeugzulassung abgewiesen. Nur-Lesen
veröffentlicht keine Schreib-/Bildwerkzeuge. Ein bereits vorhandenes Ausgabeziel
wird vor einer weiteren Freigabe als bekannte fehlgeschlagene Aktion gespeichert.
Native Prozess- und Connectorrechte werden dadurch nicht erweitert.

Die gemeinsame Engine bereitet Ergebnisse vor; der jeweilige Lauf behält sein
eigenes Wirkungsjournal. Workflow-Dateiveröffentlichung hält zuerst den originalen
Generate-Beleg samt Engine-/Vorschaumetadaten fest und veröffentlicht create-only
in der bestätigten Kopie. Nach verlorener Source- oder Graphantwort beobachtet
die Ergebnisklärung den tatsächlichen Dateibeleg, statt die Engine erneut zu starten.
Vollständig gespeicherte Dokumentlese-/Fehlerergebnisse können ebenfalls anhand
ihres ursprünglichen Belegs geklärt werden. Unzureichende Belege bleiben ungeklärt.
Die bestehenden Chat-/Werkstattpfade behalten ihre eigenen Pending-Effektbelege.

Vorschaubilder aus bestätigten Dokumentquittungen lassen sich über den autorisierten
Lauf lesen und in einen folgenden bildfähigen Modellturn übernehmen. Andere Läufe
erhalten dadurch keinen Zugriff. Enginearbeit bleibt an die verbleibende Knoten-/
Elternfrist und den Abbruch gebunden; die Engine bekommt keine neue eigene Laufzeit
über die ursprüngliche Grenze hinaus.

Der erste echte Windows-Browserdurchlauf besteht mit ausdrücklicher Enginewahl,
Planprüfung, verlorener Commitantwort, Reload des Originalziels, genau einem Lauf
und einer Freigabe, tatsächlicher DOCX-Veröffentlichung nur in der Kopie sowie
hartem eigenem Prozessneustart ohne zusätzliche Modell-/Dateiwirkung. Zwei lokale
HTTP-Modellanfragen und genau ein Generate-/Sourcebeleg; finale Instanz exit0.
390px ohne Überlauf oder Seitenfehler. Die vorherigen zwei Testaufbaufehler bleiben
getrennt erhalten. Der finale gebundene Bericht (private development evidence; omitted from public sources)
und seine Prüfsummendatei sichern den persönlichen Windows-Workflowschnitt: 145
Runtimefälle einschließlich 19 echter Dokumentfälle (11 neue Workflow- und acht
bestehende Chat-/Werkstattfälle), 31 Storagefälle, ausgewählte Formatprüfungen und
Fixturebuild bestehen auf 1212 unveränderten erfassten Backendeingaben. Sieben
Storage- und vier Runtimefälle sind regulär ignoriert. Das ist kein neuer PG-Nachweis.
590 zusammengeführte Frontendfälle, TypeScript, Webbuild und 726 Verträge bestehen
im separat gebundenen Owner-/UI-Fenster; 46 gezielte Client-/UI-Fälle dieses
Dokumentanschlusses sind ebenfalls grün. 37 unveränderte beteiligte Quelldateien,
die tatsächlichen Testprogramme und das gebaute Browserbundle sind separat erhalten.
Der spätere breite Produktvergleich weist acht fremde Änderungen im
Runner-Protokoll und dessen Daemonkonfiguration ausdrücklich aus; der Bericht
behauptet deshalb keinen unveränderten vollständigen Produktstand.

Ein zusätzlicher Rechtebefund ist geschlossen: öffentliche, interne Aufgaben-
und alte Planstarts dürfen Dokumentprozesse nicht aus bloßen Werkzeugnamen
ableiten. Sie werden vor Providerprobe und Laufanlage abgewiesen. Der V2-Resolver
reicht den privaten Dokumentzielkontext erst nach ausdrücklicher Auswahl weiter;
der Dispatch verlangt die ursprüngliche DocumentEngine-/Arbeitskopienbindung.
Bestehende Chat-/Werkstatt-Originalaktionen behalten ihren eigenen bisherigen Pfad.

Die tatsächliche Linux-Engineprobe deckte einen bestehenden Importfehler auf:
openpyxl wollte systemweite MIME-Dateien lesen, die von der ursprünglichen
Auditgrenze korrekt blockiert wurden. Die Initialisierung nutzt jetzt eingebaute
Zuordnungen ohne Host-MIME-Dateien oder Registrydaten. Je drei vorhandene
Engineproben sowie zusätzliche injizierte MIME-/Registry-Leseverbote bestehen
unter Windows und Ubuntu-24.04, ohne die Lesegrenze auszuweiten.

Die Linuxabnahme bleibt rot. Nach einem Compilerfehler im Warnungsrenderer baut
Rust 1.94.1 mit kurzer Diagnostikausgabe, weiterhin mit allen Warnungen und ohne
Quell-/Compileränderung. Zwei bestehende Storagefälle stoppen mit
WorkflowClockRollback. Die neue vollständige Dokumentsuite besteht 18/19:
zunächst eine Freigabe-Beobachtungsfrist, danach im kontrollierten Folgelauf ein
konkreter WorkflowClockRollback. Dasselbe unveränderte Test-ELF besteht einzeln
in 6,47 Sekunden; das überschreibt keinen der fehlgeschlagenen Gesamtläufe.
Ursachenannahmen für den ersten Timeout bleiben offen. Keine Testfrist erhöht,
keine Clockprüfung gelockert und keine Linux-/Runtimefreigabe daraus abgeleitet.

Der nachfolgende Einzelchatschnitt ergänzt den ausdrücklich bestätigten Dokumentziel-
Einstieg samt eigener COW-/Enginebindung. Weiter offen: modellfreie native
Dokumentbausteine mit typisierten Aktionsdaten,
eigene Vorschau-/Ergebniskarten pro Workflowknoten, OCR mit überprüfbaren Fundstellen
und Unsicherheiten, vollständige Rechnungsreferenz, weitere Ausführungsziele sowie
installierte Desktop-/Geräte-/Enterprise- und Releaseabnahme.


## Ausdrückliche Dokumentwahl im normalen Einzelchat

Stand31.10.2026: Vor der Planvorbereitung wählt der Nutzer zwischen begrenztem
Dateibroker und Dokumentengine. Der normale Chat verwendet dieselben bestätigten
V2-Eingaben und Arbeitskopien wie der bestehende Ownerpfad. Ein Prompt oder bloße
Werkzeugnamen erteilen keine Programmrechte. Ein gespeicherter Originalrequest
bleibt beim Neuladen vorrangig; eine neue Auswahl ersetzt ihn nicht.

Die Dokumentprüfung verlangt vor einer neuen Freigabe und nochmals unmittelbar
vor der Enginevorbereitung die zum ursprünglichen Plan gehörende Controller-,
Arbeitskopien- und Enginerevision. Ein Enginewechsel nach einer Freigabe erzeugt
keine Datei. Eine fehlende Engine wird vor einer Modellprobe erkannt. Bestehende
Chat-/Werkstatt-Originalaktionen ohne V2-Arbeitskopie behalten ihren eigenen Pfad.
Die zusätzliche Wahl gilt für einzelne bestätigte Starts; begrenzte dauerhafte
Automatisierungsrechte nach A05 bleiben offen.

Acht tatsächliche Runtimefälle decken Freigabe/Create-only-DOCX, verlorenes
Dateiabschluss-ACK mit Store-/Broker-Reopen und belegter Fortsetzung, geänderte
Originaldatei bei eingefrorener Bildvorschau, Nur-Lesen, fremde Ordner, fehlende
Engine, Enginewechsel nach Freigabe und den Dateibroker ohne Dokumentrechte ab.
Die Modellantworten kommen von einem eigenen lokalen synthetischen HTTP-Server;
die Dokumentengine verarbeitet echte Dateien. Das allgemeine Toolboxjobverzeichnis
ist kein Messpunkt für Dokumentengineaufrufe; der Nachweis verwendet tatsächliche
Dateieffekte, Originalquittungen und beobachtete HTTP-Aufrufe.


### Gebundener Einzelchat-Prüfstand, 31.10.2026

Der neue Sammelbericht (private development evidence; omitted from public sources) hält 153 Windows-Runtimefälle und 31 Storagefälle fest, darunter 27 echte Dokumentfälle. Die acht neuen Einzelchatfälle bestehen zusätzlich unter Ubuntu-24.04 mit der echten privaten Engine, ohne ignorierte Fälle und auf 1223 unveränderten erfassten Backendeingaben. Die ältere fehlgeschlagene Linux-Workflowqualifikation bleibt erhalten und wird damit nicht überschrieben.

594 Frontendfälle, TypeScript und Webbuild bestehen im abschließenden Fenster. Die 726 öffentlichen Verträge bleiben unverändert und stimmen überein. Der tatsächliche Einzelchat-Browserlauf bestätigt die bewusste Zielwahl, keine Ausführung vor Bestätigung, verlorene Commitantwort und Reload ohne neue Eingabecapture, eine Freigabe, eine echte DOCX und genau zwei lokale synthetische Modell-HTTP-Anfragen. Ein harter eigener Prozessneustart erhält Originalplan, Lauf und Snapshot ohne neue Datei-/Modellwirkung; 390px ohne Überlauf/Seitenfehler, finale Instanz exit0.

Neun beteiligte Featurequellen bleiben in den erfassten Fenstern unverändert und sind separat erhalten. Tatsächliche Testprogramme, QA-Daemon, Browserbundle und Berichte sind über Prüfsummen gebunden. Der breite Windowsvergleich enthält 12 parallele fremde Änderungen; im abschließenden 1659-Input-Fenster sind 11 Änderungen ausdrücklich ausgewiesen. Dieser abschließende Gesamtquellenvergleich bleibt deshalb false. Keine neue PostgreSQL-, installierte Desktop-, Geräte-, Team-, Enterprise- oder Releaseabnahme. A01–A12, T01–T10 und der Gesamtauftrag bleiben offen.

### Nativer Dokumentkern: vorbereitet, Produktanschluss noch offen

Stand31.10.2026: Die aktuelle Domain enthält ein additives optionales
`WorkflowDefinition.documents` für typisierte Format3-Abläufe und deren
bestehende eingefrorene Format4-/5-Unterabläufe und Listen. Die Aktionsdefinition
bindet Textlesen mit Fortsetzungsmarker, Vorschau und DOCX-/PDF-Erstellung,
konkrete Eingaben und feste Ergebnistypen. Die Arbeitsbereichsauswahl wird vor
der Kompilierung ausdrücklich gebunden; konkrete Ordner werden gegen die
effektiven Rechte geschnitten. Der Knoten erhält nur sein festes Werkzeug und
seinen gewählten Ordner. Er besitzt keine Modellrolle; Modellzulassung scheitert.
Ungelöste Werkzeugwirkungen blockieren weiter den Abschluss.

Die separate aktuelle Domain-/Contracts-Kopie besteht auf Windows und Ubuntu
je169 Fälle:153 bestehende und16 neue Fälle. Sie prüfen zusätzlich ungenutzte
Modellkonfiguration ohne Anbieter, unveränderte alte Planbytes/-hashes,
Eingabefeldbindungen, Ordner-/Werkzeuggrenzen, tatsächliche Knotenmaterialisierung
in eingefrorenen Unterabläufen und getrennte Listeninstanzen.116 erfasste Eingaben
bleiben während beider Prüfungen unverändert. Strenges Domainlib-Clippy und
Formatprüfung bestehen. Das reduzierte private Cargo-Workspace/Lock ist kein
Nachweis des gesamten App-Workspace; es wird nicht in die aktuelle Quelle
übernommen. Der geprüfte Domainpatch wurde gegen die unveränderten Originalhashes
abgeglichen. Aktive Rustkonstruktoren erhalten die leere neue Map, historische
Migrationsreferenzen bleiben unverändert.730 erzeugte öffentliche Verträge
enthalten die vier zusätzlichen Datentypen.

**Dieser Kern ist noch kein ausführbarer Produktbaustein.** Der aktuelle
Runtimepreflight weist neue native Dokumentknoten ausdrücklich mit
`workflow_native_document_steps_unavailable` vor Providerkontakt zurück.
Bestehende modellgeführte Dokumentaktionen bleiben erhalten. Die Wiederverwendung
von Originaljournal, tatsächlicher Engineausführung, Freigabe, Ergebnisklärung und
Editor-/Ergebniskarten ist der nächste Anschluss. Ein Toolvorschlag oder eine
Domainquittung erzeugt keine IO-Autorität. Native UserRights-Ausführung bleibt
sichtbar vom noch offenen qualifizierten Managed-Dokumenttoolpack getrennt;
Host-Python/Audithook ist kein gVisor-Isolationsnachweis. Es erfolgt keine
vorzeitige Umstellung bestehender Pipelines. OCR/Rechnungen und A01–A12/T01–T10
bleiben offen.

Für den nächsten Anschluss wird das bestehende deterministische Vorschlagsjournal
auf die neuen rollenlosen Knoten erweitert: Kontext/RunSpec ohne Provider,
fest gebundener Toolvorschlag aus den ursprünglichen typisierten Eingaben und
bestehende Toolzulassung/Freigabe/Sourcebelege. Es entsteht kein künstliches
Modellprofil. Der gespeicherte Antworttyp muss seine Definition, Ein-/Ausgaben
und vollständig belegten Ergebnisse auch nach Wiederherstellung validieren.
Die tatsächliche Enginequittung wird auf feste Datenfelder projiziert; Warnungen,
Trunkierung, Fortsetzungsmarker und Quellhash dürfen dabei nicht verloren gehen.
Die temporäre Unavailable-Grenze wird erst durch echte positive Ausführung mit
null Modell-HTTP-Anfragen und die zugehörigen Fehler-/Neustartfälle ersetzt.

### Integrierte Kernprüfung und verbleibende Grenze

Der gebundene Kernbericht (private development evidence; omitted from public sources)
verbindet die beiden unveränderten169-Fälle-Kernfenster mit dem tatsächlichen
Main-Workspace-Domainlauf (ebenfalls169 Fälle). Nach dem separat erhaltenen
fehlgeschlagenen Broker-Zwischenstand besteht der aktuelle Windows-Folgelauf mit
28 Dokument- und27 Snapshot-/Ressourcenfällen. Der neue Negativfall verwendet
eine tatsächlich eingerichtete Engine und einen eigenen lokalen HTTP-Anbieter;
der native Berichtsentwurf wird vor Modellprobe, Modellanfrage oder Laufanlage
mit der ausdrücklichen Unavailable-Grenze abgewiesen. Das ist ein geprüfter
Zwischenstand und kein positiver Nachweis nativer Dokumenteffekte.

594 Frontendfälle, TypeScript, Webbuild und der unabhängige730-Vertragsvergleich
bestehen. Das erste Main-Fenster scheiterte an drei fremden Broker-Arc-
Konstruktorfehlern; diese wurden im zuständigen Bereich korrigiert. Der erste
fehlgeschlagene Bericht wird nicht überschrieben oder als aktueller Fehler
weitergeführt. Das aktuelle1300-Input-Fenster enthält zwei spätere fremde
Resultversionsänderungen; deshalb bleibt der vollständige aktuelle Quellen-
vergleich false. Keine App-/Runtime-/Desktop-/Team-/Enterprise-/Releaseabnahme.
SQL115 und dessen Ergebnisversionen gehören zum getrennten Root-Arbeitsbereich.

### Nächster A07-Schnitt: native Dokumentbausteine

Die erste fachliche Kette soll ohne Modellkonto laufen: ausgewählte Datei samt
Inhaltsfingerprint → Text lesen → typisierte Daten prüfen → Bericht neu anlegen.
Ein zusätzlicher versteckter Modellprovider oder ein Modellschritt mit leerem
Prompt erfüllt dieses Ziel nicht. Der bestehende native Browservorschlag allein
reicht dafür nicht: dessen deklarierte Rollen werden noch als Modellprofile
aufgelöst, und seine Ausgabe ist auf Browsertext beschränkt.

Der vollständige Produktanschluss benötigt daher weiterhin:

1. Ein eigenes, begrenztes Aktionsschema mit typisierten Eingaben, festen
   Ergebnisfeldern, Rechten und Budgets. Modellrollen dürfen für reine
   Dokumentabläufe fehlen. Definitionen ohne die Ergänzung behalten ihre alten
   Bytes und Planhashes; Unterabläufe und Listen frieren dieselbe Aktion ein.
2. Ein zunächst enges Operationsangebot: Text aus einer SHA-gebundenen Datei
   lesen, gespeicherte Vorschau erzeugen und DOCX/PDF aus explizit gebundenem
   Inhalt neu anlegen. Allgemeine Programme, ungebundene Ordnerkennungen und
   beliebige Skripte sind keine Dokumentbausteine.
3. Den bestehenden bestätigten DocumentEngine-/Arbeitskopienpfad einschließlich
   Controllervergleich, Enginepin, tatsächlichen Ordnerrechten und verbleibender
   Elternfrist. Nur tatsächlich vorhandene Modellknoten dürfen Modellprofile
   verlangen oder deren Anbieter kontaktieren.
4. Native Aktionsvorbereitung im vorhandenen Graph-/Wirkungsjournal. Eine
   Vorschau ist keine Freigabe; vor einer Schreibfreigabe entsteht keine Datei.
   Nach verlorener Engine-, Source-, Graph- oder Bestätigungsantwort bleiben
   ursprüngliche Quittungen, bekannte Ergebnisse und ungeklärte Wirkungen
   erhalten. Fortsetzen erzeugt keine neue Aktionsautorität aus einer Historie.
5. Einen Editorbaustein mit ausdrücklichen Feldverbindungen, Dateiquelle und
   Ausgabe; pro Knoten lesbare Ein-/Ausgaben, Vorschau und Fehlergrund. Bilder
   und Dokumente bleiben begrenzte autorisierte Dateireferenzen.

Die Abnahme umfasst einen veröffentlichten, tatsächlich ausgeführten Ablauf
ohne Modellprofil und mit null Modell-HTTP-Anfragen; falsche Eingabetypen,
geänderte Quellhashes und fremde Ordner; Nur-Lesen und Widerruf; Freigabe und
Enginewechsel; verlorene Antworten mit echtem Store-/Broker-Neustart; Abbruch
und Frist sowie eingefrorene Unterablauf-/Listeninstanzen. Erst danach folgen
OCR mit Seiten-/Bereichsfundstellen und Unsicherheit sowie exakte Geldwerte für
die vollständige Rechnungsreferenz T01. Dieser Abschnitt ist der Arbeitsplan,
keine Behauptung bereits implementierter nativer Dokumentknoten.


## Nativer Dokumentruntime und Editor im Main, 31.10.2026

Der vorherige reine Kernstand und dessen Unavailable-Prüfung bleiben als
historische Belege erhalten. Der neue Anschluss entfernt diese vorläufige
Laufgrenze und verwendet dieselben ursprünglichen Werkzeugfreigaben,
Dateiwirkungsbelege, Ergebnisklärung und bestätigten Arbeitskopien. Native
Dokumentknoten erhalten keinen Modellanbieter und keine erfundene Modellrolle.
Die öffentliche Freigabeprojektion lässt deshalb `role_id` weg; bisherige
Freigaben mit Modellrolle behalten ihre JSON-Form. Native Kontexte besitzen
ihren eigenen Fingerprint-Namensraum; bestehende Browser-/Modellkontexte bleiben
unverändert.

Der Editor enthält einen eigenen Einstieg „Dokumentablauf ohne Modell“ mit
einer bearbeitbaren DOCX-Aktion ohne Modellkonto. Textlesen, Seitenvorschau und
DOCX-/PDF-Erstellung lassen sich mit festen Ein-/Ergebnistypen verbinden.
Native Knoten bleiben bei Entwurfsbearbeitung erhalten, zählen in eingefrorenen
Unterabläufen und Listen zu den bestehenden Grenzen und übernehmen nur ausdrücklich
ausgewählte Ordnerrechte. Ein vorher sequenzieller Ablauf erhält beim Wechsel
seine bisherigen Reihenfolgeabhängigkeiten. Fehlende oder veränderte Datenquellen
werden nicht automatisch auf andere Schritte umgebunden.

Vorschauen bewahren Quellhash, Seitenzahl, Seitenposition und Bildmaße. Die
Enginebegrenzung auf vier Vorschauen wird bei längeren Dokumenten ausdrücklich
als gekürzt ausgegeben; eine vollständig erfüllte Auswahl einzelner Seiten
bleibt vollständig. Textausgaben beachten die 16.384-Zeichen-Grenze des
Workflow-Datenfeldes und behalten einen korrekten Unicode-Fortsetzungsmarker.
Diagnosen sind begrenzt und kennzeichnen Kürzungen ausdrücklich.

Hashkontrollierte Main-Übernahme:29 eigene Dateien, keine Manifest-/Lock-,
Migrations-, Managed-Ergebnis- oder Servicekopie. Die anfänglich beibehaltenen
älteren Dateizeitstempel wurden nach erneutem Inhaltsvergleich aktualisiert,
damit Cargo die neuen Module tatsächlich neu kompiliert. Der Programmcode
blieb dabei bytegleich.

Gebundener Main-Prüfstand:
`.local/native-document-runtime-20261031/main-integration/d7031ce2-bc9c-4005-8229-93640a0fa51b/`.
43 Windowsfälle bestehen:12 native Enginefälle,12 bestehende Dokumentfälle,
16 Domainfälle und3 Freigabeverträge auf2636 unveränderten erfassten Eingaben.
608 Main-Frontendtests, TypeScript und Webbuild bestehen;730 Verträge wurden aus
Rust neu erzeugt. Der getrennte unveränderte Kern besteht204 Fälle, weitere112
Runtime-/Workflowregressionen bestehen separat. Der aktuelle Main-Beleg gilt
für sein eigenes Quellenfenster vor der nachfolgenden getrennten Owner-Lieferung.

Die12 tatsächlichen nativen Fälle enthalten bestätigte Quellkopie trotz späterer
Originaländerung, DOCX, PDF mit genau einer Freigabe, Ablehnung, Nur-Lesen,
Quellhashfehler, Enginewechsel, Bild-/PDF-Vorschauen, Unicode-Fortsetzung und
beide verlorenen Abschlussantworten mit Store-/Broker-Reopen und Fortsetzung
bei genau einer Datei. Es ist kein Modellkonto konfiguriert.

Ubuntu bleibt ausdrücklich unqualifiziert: Nach Korrektur eines Prüfskriptfehlers
(venv-Symlink darf nicht zum System-Python aufgelöst werden) bestehen zwei volle
Läufe9/12; ihre Unterbrechungen zeigen `workflow_clock_rollback` und einmal
anfängliches `budget_exceeded`. Dieselben drei Fälle bestehen einzeln mit dem
unveränderten ELF. Diese Einzelbeobachtungen schließen den roten Gesamtlauf
nicht. Keine Uhr-, Frist-, Budget- oder Freigabegrenze wurde gelockert.

Aktuelle Browser-/native GUI-/Installationsabnahme bleibt offen. Eine zusätzliche
Browserfixture ohne Modellanbieter wird separat gebaut. Native UserRights sind
weiter ausdrücklich vom qualifizierten Managed-Dokumenttoolpack getrennt;
Host-Python/Audithook beweist keine gVisor-Isolierung. Gespeicherte Aufgaben und
Ereignisregeln erhalten keine automatische Prozessfreigabe. OCR, Tabellenpfad,
Rechnungsreferenz sowie A01–A12/T01–T10 und die vollständigen Liefer-/Releasegates
bleiben offen.

## Nachfolgende Browser- und Anzeigeprüfung, 31.10.2026

Ein eigener tatsächlicher Windows-Daemon mit echter Python-Dokumentengine,
eigenem Chromiumprofil und null Modellanbietern wurde im Browser bedient:
Dokumentvorlage erstellen, Name/Datei/Inhalt bearbeiten, veröffentlichen,
veröffentlichten Stand auswählen, Dokumentengine ausdrücklich wählen und den
PlanV2 mit eingefrorener Arbeitskopie prüfen. Die Übersicht zeigte keine
Modellanfragen sowie die tatsächlichen Benutzer- und Netzwerkrechte.

Genau ein Lauf, eine ursprüngliche Freigabe, ein Wirkungsbeleg und eine DOCX-Datei
wurden gespeichert. Ein Verlust der angenommenen Freigabeantwort mit anschließendem
Neuladen erzeugte keine zweite Veröffentlichung. Ein harter Neustart des eigenen
Daemons nach Abschluss behielt denselben Lauf und dieselbe Datei. Der originale
Quellstand wurde nach der Planprüfung geändert; die Arbeitskopie behielt ihren
ursprünglichen SHA-256 und das geänderte Original blieb erhalten. Der tatsächliche
DOCX-Inhalt und der Ergebnis-SHA-256 wurden getrennt geprüft.

Der Start wurde außerdem während der ursprünglichen Bestätigung getrennt;
die Oberfläche fand beim Neuladen denselben angenommenen Lauf. Der Prüfdriver
beendete seine erste Antwortverlustmessung jedoch zu früh. Deshalb wird hierfür
keine präzise Prüfung eines erst nach HTTP200 verworfenen Startbelegs behauptet.
Der harte Browserneustart fand nach Abschluss statt, nicht während einer noch
ungeklärten Dateiwirkung. Die getrennten Backendfälle prüfen diese Wirkungsgrenzen.

Die Browserprüfung fand einen Anzeigefehler: Markdown veränderte nackte
JSON-Ergebnisse, insbesondere Unterstriche und leere Listen. Gültige JSON-Objekte
und -Listen werden jetzt als unveränderte Daten dargestellt; auch große Zahlen
werden nicht neu serialisiert. Eingebettetes Markup bleibt Text. Rollenlose
Schritte heißen allgemein „Schritt“, damit Dokumentaktionen nicht fälschlich als
Kontrollbaustein erscheinen. Zwei gezielte Rendererfälle und die tatsächliche
Browseranzeige prüfen die Korrektur. Drei UI-Dateien wurden erst nach exaktem
Vergleich der ursprünglichen Main-Inhalte übernommen.

Gebundene Folgebelege:

- Browser:
  `.local/native-document-runtime-20261031/browser-runs/ada6735e-7f14-43c3-8686-66935b0c57a6/result.json`;
  SHA-256 `6e4acec8ed370fb758d55ea373ed6be04b1d1b6aaceb442767b9e75afc162063`.
  Der Bericht erhält2638 Backend-Eingaben als gehashtes Quellenarchiv, das echte
  Programm, die ausgewählten UI-Builds, Bilder und Originalbelege. Beide eigenen
  Listener wurden geschlossen und der eigene Daemon bestätigte sein Ende.
- Isolierte Anzeigeprüfung:
  `.local/native-document-runtime-20261031/ui-runs/b91d7d44-9284-47d9-9c51-a8bac66e859b/result.json`;
  605 Tests, Typprüfung,730 Verträge und Webbuild bestehen.
- Aktuelle Main-Anzeigeprüfung:
  `.local/native-document-runtime-20261031/main-display-followup/4e8e90ff-e4db-41fb-8b9b-f017deef0fd2/result.json`;
  631 Tests, Typprüfung und Webbuild bestehen auf415 unveränderten Eingaben.
  Die bereits aktuellen735 Verträge wurden mit erfasst; kein Cargo-/Generatorlauf.

Die unabhängige Linuxdiagnose führte dasselbe unveränderte ELF erneut aus und
zeichnete99.919 UTC-/Monotonic-Proben sowie175 rein lesende Datenbankbeobachtungen
auf. Dabei sprang die echte System-UTC zweimal um854,18ms und848,41ms zurück,
während Monotonic jeweils etwa0,59ms fortschritt. Der Lauf bleibt mit11/12 rot
(`ExecutionPlanExpired`); die früheren9/12-Belege bleiben unverändert.
Beleg:
`.local/native-document-runtime-20261031/linux-clock-diagnostics/9165361d-571e-4ac4-a2db-005953692e7b/result.json`.
Das belegt eine instabile Plattformuhr, erklärt aber nicht automatisch jede
frühere Unterbrechung. Uhr-, Frist-, Budget- und Freigaberegeln bleiben wirksam;
WSL, Hostzeit und Zeitdienste wurden nicht umgestellt.

Diese enge Browserabnahme ersetzt weder die installierte Desktop-/Mobilabnahme
noch den vollständigen Linuxlauf, einen isolierten Managed-Dokumenttoolpack,
OCR, Tabellenverarbeitung oder die Rechnungsreferenz. Der vollständige Auftrag
bleibt offen.

## Priorisierte nächste Produktlieferungen

Die verbindlichen A01–A12-Anforderungen und T01–T10-Vorlagen bleiben der
Gesamtumfang. Für den tatsächlichen Nutzwert gilt folgende Reihenfolge:

1. **Zuverlässigen Ausführungspfad schließen.** Aktuelle Ziele einrichten und
   sichtbar prüfen; vollständige Windows-/Linux-Nachweise, Abbruch,
   Antwortverlust, Wiederaufnahme und Ergebnisübernahme abschließen. Die
   Plattformuhr muss für die tatsächliche Linuxabnahme stabil sein.
2. **Rechnungen vollständig verarbeiten.** Datei-/Webhookeingang, Text/OCR mit
   Seitenfundstellen, typisierte Extraktion, Pflichtfeld- und Geldwertprüfung,
   Rückfrage bei Unklarheit, bestätigte Tabellenänderung, Ablage und Meldung.
   Der Referenzablauf muss Dubletten, schlechte Scans, geänderte Quellen und
   Neustarts mit bereits bestätigten Wirkungen behandeln.
3. **Automationen einfach erstellen.** Geführte Verbindungen für E-Mail,
   Kalender, Dateispeicher und APIs; Eingabe aus einer Beschreibung als
   bearbeitbarer Entwurf; Testdaten, Schrittansicht und Test vor Aktivierung.
   Zehn nutzbare Vorlagen erhalten dieselbe Einrichtung und Fehlerbehandlung.
4. **Unbeaufsichtigt betreiben.** Eingerichteter Hintergrunddienst,
   Warteschlangen, Nachhol-/Überlappungsregeln, absolute Termine und
   Ereigniswarten; begrenzte Dauerrechte, zentrale/mobile Freigaben,
   aussagekräftige Störungsmeldungen und sichtbare Ressourcenlimits.
5. **Wiederverwendung und Teambetrieb ausbauen.** Stabile aufgezeichnete
   Browser-/Desktopbausteine, belegte Wissenssuche mit Aktualisierung,
   Besitzerübergabe, Entwurf/Test/Produktion, Versionsvergleich,
   Wiederherstellung und signierte Updates. Jede Plattform und der getrennte
   Enterprise-Server erhalten ihren eigenen tatsächlichen Abschlussnachweis.

Jede Lieferung benötigt mindestens einen vollständig bedienbaren Referenzablauf.
Zur Freigabe gehören verständliche Ergebnisansichten, überprüfte Fehlerfälle,
Neustart ohne doppelte bestätigte Wirkung und passende Rechte am tatsächlichen
Ausführungsziel. Automatisch wiederholt werden nur nachweislich geeignete
Aktionen; ungeklärte externe Wirkungen benötigen Ergebnisklärung.
