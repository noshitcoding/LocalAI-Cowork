# Unterhaltung, Zweige, Eingang und Export

Der aktuelle Dienst verwendet weiterhin ein lokales Eigentümerkonto. Die neuen APIs
sind durch denselben privaten Daemon-Schlüssel und dieselbe Origin-Prüfung geschützt.
Eine zukünftige Team-API muss ihren Inhaltszugriff vor diesen Datenbankoperationen
prüfen; die globale lokale Suche ist keine implementierte Mandantentrennung.

## Dauerhafte Reihenfolge und Suchindex

Migration 2 ergänzt eine Nachrichtenposition je Auftrag. Bestehende Nachrichten
behalten die zuvor angezeigte Reihenfolge nach Erstellungszeit und ID. Neue Nachrichten
werden unter einer Auftragssperre angehängt. Nachricht, Suchdokument, Laufzustand und
Ereignis werden gemeinsam bestätigt. Ein fehlgeschlagener Schreibvorgang hinterlässt
keinen alleinstehenden Suchtreffer.

Die Suche verwendet einen gemeinsamen portablen Index für SQLite und PostgreSQL.
Titel, Nachrichten und Textartefakte werden in Unicode-Kleinschreibung durchsucht.
Prozentzeichen, Unterstriche und das SQL-Escapezeichen werden wörtlich gesucht.
Es gibt maximal 100 Treffer pro Seite. Das ist eine Teilzeichenfolgensuche, keine
semantische Suche oder ein Nachweis der Enterprise-Lastziele.

## Zweige

`POST /api/v2/orders/{order}/branches` erhält Titel, letzte Assistant-Nachrichten-ID
und Idempotenzschlüssel. Ein Zweig bleibt im Projekt der Quelle. Bis zur gewählten
Antwort werden Nachrichten und Ergebnisse der darin abgeschlossenen Antworten als
unveränderlicher Kontext gespeichert. Original-IDs, Original-Eigentümerschaft und
Artefakthashes bleiben bestehen; der Snapshot hat einen eigenen SHA-256-Wert.

Neue Nachrichten und Läufe gehören ausschließlich zum neuen Auftrag. Freigaben,
offene Rückfragen und laufende Werkzeugaufrufe werden nicht übernommen. Der erste
neue Lauf erhält den historischen Textkontext, benötigt aber eine explizite
Modellauswahl und eine neue Laufkonfiguration. Die Herkunft ist in der Oberfläche
sichtbar. Auch ein Zweig kann erneut verzweigt werden.

Verwaltete Arbeitsdateien werden am Laufabschluss als gehashter Dateisnapshot erfasst.
Ein Zweig erhält eine getrennte Kopie des Dateistands an seiner gewählten Antwortgrenze.
Externe Ordner werden nicht kopiert. Ältere Preview-Ergebnisse ohne Dateisnapshot starten
mit einem leeren Arbeitsverzeichnis. Siehe [WORKSPACES.md](WORKSPACES.md).
Schreibende Terminals und qualifizierte Ausführungs-Sandboxes sind noch offen.

## Eingang und Aktivität

`GET /api/v2/activity` liefert offene Läufe mit Projekt- und Auftragstitel. Der Eingang
beschränkt auf `waiting_approval`, `waiting_input` und `interrupted`. Das Öffnen eines
Eintrags wählt genau dessen Lauf. Eine Entscheidung erfolgt anschließend in der
Auftragsansicht mit den konkreten Werkzeugargumenten. Das globale Aktivitätssymbol
bleibt beim Projektwechsel sichtbar.

## Auftragsexport

`GET /api/v2/orders/{order}/export` liest Auftrag, bestätigten Entwurf, native und
geerbte Unterhaltung, Textartefakte, Laufkonfigurationen, Freigaben und Ereignisse
aus einem Datenbank-Snapshot. PostgreSQL verwendet dafür Repeatable Read. Größe und
Hash der Artefakte sowie die lückenlose Ereignissequenz werden vor Ausgabe geprüft.
Credentials und interne Worker-Leases sind keine Exportfelder.

`format_version = 7` enthält Textdatei- und Verzeichnisoperationsbelege, modellfreie Wiederherstellungsläufe mit Herkunft, Upload-Metadaten, Modellversuche und Tokenreservierungen, unbestätigte Änderungen und Workspace-Snapshots einschließlich geerbter historischer Manifeste. Private Dateiversionsinhalte gehören nicht zu diesem JSON-Export. `sha256`
ist SHA-256 über UTF-8-JSON des Feldes `data`, mit rekursiv lexikografisch sortierten
Objektschlüsseln und ohne zusätzliche Leerzeichen. `exported_at` gehört nicht zum
Hash. Der Systemtest berechnet diesen Wert mit Python unabhängig neu.

Der Export ist zur Einsicht und Erhaltung einer Unterhaltung bestimmt. Er ist kein
vollständiges Sicherungsformat, kein M9-Migrationspaket und kann keine Läufe starten.
Die Web-Oberfläche bietet einen JSON-Download. Dessen tatsächliche Dateiablage im
eingebetteten Browser und in der Desktop-Hülle ist noch gesondert zu qualifizieren.
