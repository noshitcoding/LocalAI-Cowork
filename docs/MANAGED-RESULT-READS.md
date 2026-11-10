# Frische Leserechte für Ergebnisse der eigenen Runtime

Der Controller liest Ergebnisdateien über einen eigenen, kurz gültigen privaten
Transportpfad. Die ursprüngliche Abschlussquittung bleibt Ausführungshistorie.
Sie verlängert weder Eingaberechte noch Workerlease und erzeugt keine weitere
Programmausführung. Der [Owner-Anschluss für lokalen Abruf und Download](MANAGED-OWNER-RESULTS.md)
ist inzwischen integriert. Die ausdrückliche Ergebnisübernahme und die aktuelle
installierte App-Abnahme bleiben gesondert offen.

## Erneute Autorisierung

`Runtime::managed_result_manifest` und `managed_result_chunk` sind ausschließlich
für den vertrauenswürdigen lokalen Owner-Aufrufer bestimmt. Sie akzeptieren keine
vom Renderer behauptete Actoridentität, Quittung oder native Dateiberechtigung.
Der Store löst den vorhandenen Auftrag und Lauf, den bestätigten Plan, den
ursprünglichen Eingabetransfer und die signierte Abschlussquittung erneut auf.
Der Prozessversuch muss bereits als abgeschlossen im ursprünglichen Journal
stehen. Ein beobachteter Abschluss vor dieser Journalisierung reicht nicht.

Aktuelle Controllergeneration, registriertes Ziel und dessen Revision sowie die
qualifizierte Umgebung müssen weiterhin zum bestätigten Plan passen. Der native
Broker prüft die tatsächliche Identität der bestätigten Arbeitskopie und ihre
ursprünglichen Ordnerrechte. Während eines Abrufs hält er die Ressourcen- und
Kopie-Locks; nach Transportantwort prüft der Controller Abbruch und Autorität
erneut. Ein abgebrochener Lauf liefert über diesen Pfad keine neuen Bytes.
Ein abgeschlossener Lauf benötigt dafür keinen wiederbelebten Worker.

Die Actorkennung stammt aus dem tatsächlichen Installation-Owner-Controller.
Ihr Autorisierungsdigest bindet die überprüften Inhalte und dient dem Audit;
ein Digest allein ist keine Berechtigung. Team- und Geräteprincipal müssen vor
einer späteren Nutzung über ihren eigenen Autorisierungspfad aufgelöst werden.

## Privater Transport

Vor jedem Abruf beantwortet der konfigurierte HTTPS-Runner eine frische signierte
Bereitschaftsanfrage. Sie bindet die aktuelle native Runtime-Instanz. Der neue
[frische Lesekontext](MANAGED-RESULT-CONTEXT.md) beobachtet anschließend den aktuellen
Fence und bindet dieselbe ursprüngliche Abschlussquittung. Danach signiert der
Controller einen Format-2-`managed_result_read`-Request mit diesem Kontext und eigener Kennung,
höchstens 30 Sekunden Gültigkeit, ursprünglichem Plan, Run, Versuch, Launch-
Envelope und Hash der ursprünglichen signierten Abschlussquittung.

`POST /v2/managed/results/{attempt}/read` kennt zwei Operationen:

- `manifest` liefert das portable Manifest und die signierte Output-Attestation.
- `chunk` bindet zusätzlich genau diese Attestation, das Manifest und einen darin
  deklarierten Pfad, Offset und Chunkhash. Die Binärantwort darf höchstens die
  deklarierte Länge bis zu 4 MiB haben und muss denselben SHA-256 tragen.

Der Controller prüft Outputsignatur, native Instanz, ursprüngliche Launch-,
Input-, Plan- und Quittungsbindung, Aufbewahrungsfrist und alle erlaubten
Kopie-Aliase. Er verwendet die bestehende Begrenzung aktiver Transfers. HTTP-
Weiterleitungen, Proxy und automatische Wiederholung sind deaktiviert. Eine
verlorene Antwort erzeugt keinen zweiten Programmstart. Ältere Transporte ohne
diesen eigenständigen Pfad lehnen die Anfrage standardmäßig ab.

## Noch erforderliche Integration

Der quellgebundene Prüfstand (development artifact omitted from public sources) umfasst 27
SQLite-Fälle und dieselben 27 Fälle mit eingefrorenem Testprogramm auf jeweils
einer eigenen PostgreSQL-Datenbank. Alle Datenbanken sind nachweislich entfernt.
14 getrennte HTTPS-Clientfälle prüfen unter anderem tatsächliche Loopback-TLS-
Verbindungen mit synthetischem signiertem Peer. Ein durch die unabhängige
Quellenprüfung gefundener Retention-Fehler ist korrigiert und mit einer tatsächlich
verzögerten frischen Umgebungsantwort geprüft. Dieser historische Prüfstand bleibt
erhalten; spätere lokale Ergebnisversionen und der öffentliche Owner-Anschluss
haben eigene Prüfbelege (development artifact omitted from public sources). Das behaltene
Entwicklungsprofil wurde zuletzt auf Schema 113 geprüft; der aktuelle
Schema115-Quellenstand ist darin weiterhin nicht abgenommen.

Der [private native Empfängerkern](MANAGED-RESULT-STAGES.md) ist inzwischen getrennt
implementiert und auf Windows sowie WSL/Linux geprüft. Er bilanziert begrenzte
Empfangspacks einschließlich Teilübertragungen über Neustarts hinweg. Die
[Anbindung lokaler Ergebnisversionen](MANAGED-RESULT-VERSIONS.md) empfängt Dateien
in einer getrennten privaten Version mit atomarer Ready-Veröffentlichung.
Teilübertragungen erscheinen nicht als fertiges Ergebnis und behalten ihre
Kapazitätsbuchung bei einem Neustart.

Die Übernahme benötigt eine eigene Vorschau und ausdrückliche Bestätigung. Ihr
Vergleichsstand ist der tatsächlich übertragene Eingabesnapshot. Zwischenzeitliche
lokale Änderungen müssen Konflikte ergeben und erhalten bleiben. Die reine
Ergebnisübertragung schreibt keine Originaldatei und bestätigt keine Übernahme.
Öffentliche DTOs, authentifizierte Owner-Routen und die Ergebnisansicht verwenden
diesen getrennten Versionspfad. Die ausdrückliche Übernahme benötigt weiterhin
ihre eigene Vorschau und Bestätigung.

Der neue [Format-2-Lesekontext](MANAGED-RESULT-CONTEXT.md) stellt die gesonderte
frische Inhaltsautorität nach einem physischen Stopp oder einer späteren
Ausführungsepoche bereit. Eine tatsächliche native Probe belegt diesen Ablauf;
die ursprüngliche Ausführungslease wird dabei nicht wiederbelebt. Installierte
App-Anbindung, Ergebnisfreigabe, Workflow-/Aliasrechte und vollständige
Plattformabnahme bleiben gesondert offen.
