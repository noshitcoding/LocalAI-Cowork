# Empfangsbestätigung und Runner-Speicherfreigabe

Dieser Anschluss ist im aktuellen App-Quellstand integriert. Die ausdrückliche
Bestätigung und die Freigabe zweier ursprünglicher Runner-Volumes bestehen in
einer getrennten tatsächlichen WSL2-/gVisor-Probe mit zwei Aufträgen. Die
installierte App und vollständige Laufzeitpakete sind weiterhin nicht abgenommen.

## Bedienablauf

Nach einem vollständigen, geprüften lokalen Ergebnisabruf lässt sich die
Speicherfreigabe vorbereiten. Die Ansicht zeigt die konkrete lokale Version,
Dateianzahl, Datenmenge, Manifest und die ursprüngliche Bestätigung. Erst die
ausdrückliche Empfangsbestätigung darf dem Runner erlauben, seine Ausgabekopie
freizugeben. Die lokale Ergebnisversion bleibt erhalten.

Eine verlorene Antwort lässt die ursprüngliche Bestätigung gespeichert. Neuladen
sendet keine neue Bestätigung. Der Knopf zur Quittungsprüfung liest ausschließlich
eine bereits vorhandene ursprüngliche Quittung. Diese Prüfung bleibt auch nach
Abbruch oder Widerruf des Dateizugriffs erreichbar. Fehlt die Quittung oder ist ihre
Herkunft ungewiss, entsteht daraus keine neue Bestätigung und keine Aussage, dass
die ursprüngliche Anfrage niemals angenommen wurde.

Eine ausdrücklich wiederholte Bestätigung verwendet dieselbe ID, denselben
Fingerabdruck und dieselbe ursprüngliche Annahmefrist. Sie verlangt erneut eine
vollständig geprüfte lokale Kopie und aktuelle Dateirechte. Eine neue Bestätigung
ist erst möglich, wenn die ursprüngliche Anfrage nachweislich unangenommen
abgelaufen ist und der Nutzer erneut die Vorbereitung auslöst.

Empfangsbestätigung und tatsächliche Speicherfreigabe werden getrennt angezeigt:

- Vorbereitet: Die Bestätigung wurde noch nicht versandt.
- Gesendet: Die ursprüngliche Anfrage ist gespeichert; ihre Annahme ist ungewiss.
- Angenommen: Eine ursprüngliche signierte Empfangsquittung liegt vor.
- Freigabe ausstehend oder unbestätigt: Speicher gilt weiterhin als belegt.
- Physisch freigegeben: Ein gesonderter, ursprünglicher Freigabebeleg liegt vor.

Die Statusprüfung einer angenommenen Bestätigung liest nur Freigabehistorie. Sie
sendet keine Empfangsbestätigung. Das Entfernen einer lokalen Ergebnisversion und
die Übernahme in neue Arbeitsdateien gehören nicht zu diesem Anschluss.

## Technische Grenzen

Migration116 ergänzt ein eigenes Zustelljournal; bestehende Tabellen und
Migrationsbelege bleiben erhalten. Vorbereitung, Commit und Quittungen ändern
weder den ursprünglichen Laufabschluss noch seine Prozess- oder Ausführungsrechte.
Das öffentliche API enthält ausschließlich zugeordnete IDs, Hashes, Zeitangaben
und Zustände. Signierte native Anfragen, Pfade und Besitzfähigkeiten bleiben privat.

Der getrennte native Endpunkt `/delivery/history` darf ausschließlich vorhandene
Quittungen nachschlagen. Er darf weder eine Anfrage erstmals annehmen noch einen
Ablauf klassifizieren, Speicher reservieren oder physische Ressourcen freigeben.
Historische Quittungen werden am ursprünglichen Annahmezeitpunkt geprüft. Eine
fehlende oder ersetzte Historie bleibt ungewiss.

Netzwerkduplikate sind auch bei einem einzelnen Oberflächenaufruf möglich. Sie
müssen dieselbe unveränderte Bestätigung treffen und dieselbe Quittung erhalten.
Ein beobachteter Verbindungsabbruch vor den HTTP-Headern erzeugte in der
Browserprüfung zwei identische Übertragungen. Die kontrollierte Probe mit Verlust
des begonnenen Antwortbodys prüft zusätzlich die Wiederherstellung nach Neuladen
und Ablauf der tatsächlichen ursprünglichen Frist. Beide Belege bleiben getrennt
erhalten; die Browserprobe verwendet eine synthetische API und ersetzt keine
native Freigabeabnahme.

## Prüfstand

Der zusammengeführte Main besteht89 Runtimefälle bei unveränderten2829 Quellen.
Alle88 datenbankgestützten Fälle bestehen zusätzlich auf PostgreSQL mit demselben
archivierten Testprogramm. Die Windows-Dateisperrenprüfung ist ein eigener
Dateisystemfall. Alle88 eigenen Prüfdatenbanken sind entfernt und ihre Abwesenheit
ist nachgeprüft.27 lokale TLS-Fälle, sieben Migrationsfälle und zwei Owner-Routen-
fälle bestehen auf separat archivierten, unveränderten Main-Quellen.

704 Frontendtests, Typprüfung und Web-Build bestehen bei unveränderten491 Quellen.
743 Verträge wurden aus dem zusammengeführten Main erzeugt. Die bewachte
Integration und der einzige Clientindex-Merge sind unabhängig nachgeprüft;
aktuelle OCR-, MCP- und Anmeldedateien bleiben erhalten.

Eine getrennte tatsächliche Probe führt zwei verschiedene Owner-Aufträge über
Controller, Store, FileBroker, HTTPS und gVisor aus. Pro Auftrag werden beide
ursprünglichen Volumes nach signierter Bestätigung freigegeben:48MiB und zwei
Volumes gehen auf einen leeren Pool zurück. Originale native Prozessframes,
zwölf Mutationsbestätigungen pro Auftrag, Exit0, ursprüngliche Zustell- und
Freigabequittungen sowie tatsächlich zurückgegebene Dateisystemblöcke sind geprüft.
Nach jeder Bereinigung werden alle bis dahin vorhandenen lokalen Ergebnisse,
Originaldateien und bestätigten Arbeitskopien erneut verglichen; nach dem zweiten
Auftrag auch der erste. Kein Ergebnis wird dabei implizit übernommen.

Diese native Probe bindet ihren eigenen930-Quellen-Stand und848-Quellen-
Controllerbuild. Sie ersetzt keine aktuelle installierte App- oder Plattformabnahme.
Die gemountete Browserprüfung verwendet weiterhin eine synthetische API; sie
prüft verlorene Antwort, ursprüngliche Frist, Neuladen und getrennte reine
Quittungs-/Statusabfragen. Frühere private und fehlgeschlagene Generationen bleiben
mit ihren jeweiligen Grenzen erhalten; die neu ergänzten Prüfungen nach GC werden
nicht rückwirkend früheren Läufen zugeschrieben.

Ergebnisübernahme, lokale Ergebnislöschung, allgemeine Wiederaufnahme nativer
Besitzrechte nach einem Dienstabsturz, der frühere Missing409, vollständige
Laufzeitpakete und installierte App-Abnahme bleiben offen. Die strenge
Workspace-Clippy-Gesamtabnahme ist ebenfalls noch offen. Der begrenzte
Prüfnachweis (development artifact omitted from public sources) schließt den Gesamtauftrag
A–I, Enterprise und Android nicht ab.
