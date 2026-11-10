# Vollständige Umsetzung des offenen V2- und Serverumfangs

Aktiver Auftrag: **alle offenen Punkte umsetzen**. Ein bestandener Teilabschnitt
schließt diesen Auftrag nicht ab. Maßgeblich sind die beiden Übergaben vom
9. Oktober 2026, die Feature-Matrix, die Architektur und der Gesamtplan M0–M10.

Die bisherigen Belege bleiben unverändert und gelten ausschließlich für ihre
archivierten Quellen. Änderungen seit dem Human-Resume-Abschluss benötigen neue
Nachweise; alte grüne Berichte qualifizieren sie nicht automatisch.

## Verbindliche Lieferungen und Abschlussnachweise

| Bereich | Vollständige Lieferung | Erforderlicher Nachweis |
| --- | --- | --- |
| M0 | V1-Bestandsaufnahme, installierte Versionen/Artefakte, Daten-/Policy-Matrix, reproduzierbare Builds | geprüfte Quell-/Artefaktbindungen und tatsächliche Build-/Datenmatrix |
| M1 | gemeinsamer Rust-Kern, React/Web, Tauri Windows/Linux, CI und Paketierung | ausgeführte CI und Plattform-/Installationsprüfungen |
| M2 | Einzelagenten, exakte Tokenisierung, Kontextkompaktierung, versionierte Preise, Geld-/Token-/globale Zulassungsbudgets | Modell-/Tokenizerbindungen, dauerhafte Reservierungen, Konkurrenz-/Abbruch-/Replaytests |
| M3 | vollständige Einrichtung, native Ordner-/Schlüsseldialoge, Import/Export/Dateiablage | tatsächliche Windows/Linux-Dialog- und Persistenzabnahme |
| M4 | qualifizierte Sandbox-/Runnerintegration, vollständige Dateien, Terminal, Browser, MCP, Office/PDF, GitHub | reale Werkzeugläufe, negative Isolationstests, exakte Freigaben, harte Abbrüche und belegter Abgleich |
| M5 | externe Workflowwerkzeuge, Hierarchie, sichere allgemeine Wiederaufnahme/Abbruch, Skills, Wissen/Memory, Zeitplanung, Umgebungsinstallation und Journalbereinigung | gesamte Workflowgraphen, unklare Wirkungen, Rollen-/Budgetgrenzen, DST/Überlappung und keine automatischen Replays |
| M6 | Organisationen, Identität, Rollen und Inhaltsrechte, Quoten, S3, Compose-Lebenszyklus | negative Tenanttests, getrennte Verwaltungs-/Inhaltsrechte, atomare Limits und realer Lifecycle |
| M7 | Teamsynchronisation, Replikation, Konflikte, Geräte-/Offline-Freigaben und Budgetabgleich | Offline-/Widerruf-/Konflikt-/Doppelzustellungsprüfungen ohne Rechte- oder Budgetgewinn |
| M8 | Kubernetes, gVisor, HA, Backup/Restore, Update/Rollback/Löschung | tatsächliche Cluster-/Isolations-/Failover-/Wiederherstellungsabnahme |
| M9 | vollständige vierstufige Migration und Android-/Gerätekompatibilität | alle unterstützten Quellversionen, Integritäts-/Provenienzabgleich und Protokollnegotiation |
| M10 | 100-Sitzungen/40-Läufe, 60-Minuten-Mischlast, 24-Stunden-Test, NVDA/WCAG, Installer und Release-Abnahme | tatsächliche Last-/Barrierefreiheits-/Installationsläufe, signierte Artefakt- und Updateketten |
| Server-Import | Same-Organisation-Import in neue bereits hostgebundene Instanz, dauerhafte Sperren, originale Backup-Provenienz, frische Credentials/Epochen, pausierte Zeitpläne und expliziter Cutover | echte DB-/Runnerintegration, Fehler-/Konkurrenz-/Rollbacktests und getrennte physische Cutoverbelege |
| Server-Hosted | vollständiger Hosted54→59-Lifecycle und Manager-/Gastausführung | echter KVM-/AppArmor-/Runner-Lifecycle mit positiven Cleanupzeugen |
| Server-Flotte | Ressourcenlimits, unveränderliche Hostbindung, sichere Migration der sieben Instanzen/vier Organisationen | aktuelles Inventar, vollständiges Budgetmodell, qualifizierter Import und ausdrücklich freigegebener Cutover |
| Gemeinsames System | V2 und Server mit gemeinsamer Autorisierung, Ausführung, Ereignissen, Speicher und Wiederherstellung | echte End-to-End-Prüfungen statt getrennter Kern-/Fakeprüfungen |

## Erhaltene Grenzen

Kein macOS, keine neue Android-Oberfläche, kein öffentlicher SaaS-Dienst und keine
automatische VM-Provisionierung. V1 und vorhandene Live-Installationen bleiben
erhalten. Produktive Migration, Veröffentlichung, Commit und Push sind gesonderte
Operationen; der Entwicklungsauftrag ersetzt keine konkrete Rolloutfreigabe.

Keine pauschalen Docker-/WSL-Neustarts, Prune-Aufrufe, fremden Prozesskills oder
rekursiven Löschungen. Nur exakt eigene Fixtures dürfen über geprüfte Verfahren
bereinigt werden. Gestoppte VMs bleiben bis zum positiven physischen Löschbeweis
voll gebucht. Der geschützte Pilot wird nicht umgewidmet.

Die ursprünglichen Workflowrechte, Versionen, Umgebungen, Starts, Fristen,
Budgets, Reservierungen und Quittungen bleiben gebunden. Unbekannte externe
Wirkungen werden niemals automatisch wiederholt. Resume repariert keine Clock
und erweitert keine allgemeine Cancellation-/Recovery-Ausnahme.

## Laufende Umsetzung

Der konkrete Implementierungs-, Prüf- und Fortsetzungsstand steht in
[CONTINUATION-2026-10-09.md](CONTINUATION-2026-10-09.md). Die vorherige Coreprüfung
ist archiviert; die Runner- und Quotaprüfung ist auf 216 Corequellen
im historischen Sammelbeleg (private development evidence; omitted from public sources)
versiegelt. Ihr Snapshot besteht unter Windows/Ubuntu 471/452 Rusttests.
Der neuere Prepared-Request-Snapshot besteht jeweils 238 Tests, Format, Clippy
und Daemonbuild sowie drei tatsächliche Workflow-PGfälle. Laufende Änderungen
an persönlichen gemeinsamen Geldbudgets benötigen neue eigene Nachweise;
keiner dieser älteren Belege qualifiziert sie. Produktive Runnerisolation,
vollständige Tokenizerparität und Organisationsbudgets bleiben offen.
Der Gesamtauftrag ist nach Fortsetzung wieder aktiv und bleibt fachlich offen.

1. V2: persistente Organisationen, Identität, Inhaltsrechte und atomare Quoten
   in `crates/identity` sind mit zentraler Migration und Tenantprojekt-API
   integriert. Organisationsaufträge/-läufe, gemeinsame Produktbudgets,
   Oberfläche und Deployment bleiben offen.
2. V2: bestehende Umgebungs-/Prozessjournale mit Produktstart, ausgewählten
   Umgebungen, authentifizierter Runnerautorität und Werkzeugpfad verbinden;
   Isolation vor tatsächlicher Freischaltung qualifizieren.
3. Server: Same-Organisation-Importvertrag ist an Management-/Storage-Dienste
   angebunden. Zwei aktuelle private PostgreSQL-Verträge, 55 echte HTTPprüfungen
   und 199 Tests der vier Ausführungspakete samt Clippy bestehen. Session- und
   Tokenbindung werden nach echten Lockwartezeiten und vor Commit erneut geprüft.
   Neuere Snapshotprüfungen bestehen unter Windows/Ubuntu jeweils 199 Rusttests,
   Clippy, globalen Formatcheck und nativen Windowsbuild. Die Managementimport-
   Oberfläche und ihr Routeclient bestehen 707 Frontendtests mit TypeScript,
   Styles, 160-Operationen-Vertragsprüfung und Webbuild. Die aktuell gebundenen
   Zielprüfungen umfassen echte Original-age-Dekodierung, Workspace-/S3-Readback,
   frische Credentialvergleiche und negative importierte Runaktivierung.
   Ein separater Chromiumlauf gegen den echten Managementrouter/PostgreSQL
   besteht sechs Ablaufgruppen, einschließlich verlorenem Prepare-ACK,
   identischem ausdrücklichem Retry und getrenntem Routen-CAS. Die privaten
   Betreiberqualifikationsphasen sind synthetisch; 45 eigene Browserprozesse
   sowie API/PG/WSL sind positiv beendet. Der gemeinsame Beleg archiviert
   689 Quellen. Dieser Browserstand ist historisch gegenüber der neuen
   Fleet-Generationsbindung. Deren separater 255-Quellen-Beleg besteht vier
   tatsächliche PostgreSQL-Fälle einschließlich 14 Reserve-/Release-HTTPprüfungen,
   200 Rusttests, Clippy und die aktuellen Import-/Pythonregressionen.
   Ursprüngliche reservierungsgebundene v1-/v2-Abschlüsse bleiben erhalten;
   alte Schlüssel erteilen nach Generationswechsel keine neuen Rechte.
   Die anschließende Launch-/Contentprüfung besteht 39 echte HTTPfälle mit fünf
   bestätigten PostgreSQL-Lockwartezeiten. Der aktuelle gemeinsame Beleg
   `instance-import-current-fleet-interface-sealed-9b4af5f0801f4e46978fa73af7f55c10`
   bindet 755 unveränderte Quellen, 299 Zeugendateien, jeweils 200 Rusttests unter
   Windows/Ubuntu und 707 Frontendtests. Ein frischer Browserlauf gegen echten
   Managementrouter/PostgreSQL besteht sechs Gruppen mit genau einem Import,
   Originalquittung bei identischem Replay und getrenntem Routen-CAS; das Ziel
   bleibt gestoppt. Seine 45 eigenen Browserprozesse sowie API/PG/WSL sind positiv
   beendet. Alle Archivbytes, Hashes, CRCs und aktuellen Quellen wurden unabhängig
   geprüft. Betreiber-/Enrollmentvoraussetzungen bleiben ausdrücklich synthetisch.
   Vollständiger physischer Import und gemeinsame Systemabnahme bleiben offen.
4. M9: der inerte historische Verifier ist jetzt als reines Storage-Modul
   integriert. Der auf seine damaligen Coreeingaben gebundene Produktbeleg
   `.local/history-storage-product-seal-cf4564be7c664549b08b0ee8c0f12870/seal.json`
   bindet 1873 Eingaben, 36 eingebettete Seeds und 1630 synthetische Fixtures.
   Tatsächliche Windows30-/Ubuntu29-Tests, Clippy und Format bestehen; alle
   eigenen Prozesse enden positiv. Es bestehen weiterhin keine DDL22,
   HTTP-/Produktimportoberfläche oder Live-Commitfunktion.
   Das lesende versionierte SQLite-Inventar und der freigegebene historische
   Export bestehen 18 native Windowsprüfungen mit tatsächlichem WAL und
   Dateifences. SQLite öffnet ausschließlich private Kopien. Der neue Linux-
   CLIpfad ergänzt native Read-Leases, dirfd/inotify, No-replace-Publikation und
   Stage-3-Dateifences; ein enger tatsächlicher WAL-/Writer-/Namespace-/
   Quarantäneprobe besteht zehn Beobachtungsgruppen. Das ist keine erneute volle
   Versionsmatrix oder Installationsabnahme. Der getrennte Offline-Schritt 3 besteht 31
   Windowsprüfungen auf 34 Eingaben; sein unveränderlicher Quarantäne-/Previewbeleg
   bewahrt historische Daten und öffnet niemals originale SQLite-Quellen.
   Die produktive V2-Integration mit aktueller Autorität, atomarer Speicherquote,
   Schritt 4 und Android bleiben offen; historische Inhalte verleihen keine
   Ausführungsrechte. Siehe [QUARANTINE.md](QUARANTINE.md).
5. Persönliche Geldbudgets: privater Betreiberkonfigurationsweg im Daemon vor
   Workerstart und automatische Bindung neuer Einzel-/Workflowläufe im selben
   Erzeugungstransaktionsschritt sind integriert. Zentrale Migration 0021
   sperrt alte Schema20-Daemons; ursprüngliche Ledger-/Operations-/Bindingbytes
   bleiben erhalten. Die lesende Status-API und DE/EN-Oberfläche sind integriert;
   große Beträge bleiben exakte Dezimalstrings. Die unveränderte Oberfläche
   besteht 202 Tests. Nach zwei Corekorrekturen bestehen die aktuellen
   optimierten Daemon-/Brokergates; Desktop-, Paket- und Browserprüfungen
   werden mit den frischen Releaseartefakten abgeschlossen;
   ältere Geldbudgetbelege gelten nur für ihre ursprünglichen Quellen.
6. Installierbare Pakete: aktuelle native Windows-/Linux-Releaseartefakte,
   Windows-NSIS und Linux-DEB/AppImage sind erzeugt. Paket-/Payloadquellen
   bleiben bytegebunden; Windows besteht echten WebView-Start, lokalen Modelllauf
   und lokalen/Server-Verbindungswechsel. Das aktuelle Linux-Serverbundle
   besteht private Installation und zwei geordnete Worker-Lifetimes. Native
   Linux-Fenster-/Runtimebelege werden mit ihrem tatsächlichen Umfang geführt.
   Eine spätere eigene Serverinstallation und Veröffentlichung als Alpha/Preview
   sind vorbereitet; Signierung, öffentlicher Upload, normaler Installerlauf
   und vollständige M1-/M10-Abnahme stehen aus. Siehe
   [aktuelle Installationslieferung](INSTALLATION-DELIVERY-2026-10-10.md).
7. Danach übrige Zeilen weiter umsetzen und gegen deren tatsächlichen Umfang
   prüfen. Docker-Fehler 1920 blockiert die Hosted-Liveprüfung, nicht diese
   übrigen Implementierungen.

## Persönliche Android-Serverläufe, Quellenstand 12. Oktober 2026

Migration 0051 ergänzt die bereits getrennte, benutzereigene Serverqueue aus
0050 um ein verschlüsseltes Mehrschrittjournal und einen eigenen Datenbank-
Arbeitsbereich. Der Linux-Worker verwendet die tatsächliche erstellende Sitzung,
Projekt- und Threadbindung, freigegebene Taskversion und eingefrorene
Providerbindung. Er übernimmt weder den nativen OwnerRuntime noch eine
synthetische Team- oder Geräteidentität. Schlüssel kommen aus dem bestehenden
Team-/Server-KeyStore; native Desktop-Key-Recovery ist davon getrennt.

Modellabsendungen werden vor IO genau einmal zugelassen. Originale empfangene
Antworten bleiben auch bei anschließenden Schema-/Autoritätsfehlern erhalten.
Dateiaktionen und ihre Originalquittungen werden atomar mit dem nächsten
Checkpoint geschrieben. Unterstützt sind Lesen, Auflisten, Stat, Schreiben,
Verzeichnisse, Umbenennen, Entfernen mit Originalaufbewahrung, Wiederherstellen,
Textartefakte und Benutzerfragen. Jede Änderung wartet auf die konkrete
Android-Freigabe; deren Fingerprint bindet Lauf, ursprüngliche Modellantwort,
Aktion und Policy. Rückfragen und Freigaben nutzen die vorhandenen V1-Routen.
Nach Krypto-/Schlüsselzugriffen wird die Datenbankzeit erneut geprüft.

Der Abschluss veröffentlicht in derselben Transaktion einen verschlüsselten
Ergebnissnapshot, Dateidiff und kumulierte tatsächlich gemeldete Tokenzahlen.
Fehlende Messungen bleiben ausdrücklich unvollständig; Kosten einzelner
Providerantworten bleiben in ihren Originalbelegen und werden nicht ohne
gesicherte Währungs-/Preisbindung addiert. Private Artefakte können nur mit
aktueller Eigentümerautorität heruntergeladen werden. Nach Workerverlust werden
nur vollständig geprüfte, bekannte Checkpoints wieder eingereiht. Ein
zugelassener oder unbekannter externer Effekt wird niemals durch Neustart erneut
gesendet; nicht vollständig im Checkpoint veröffentlichte Empfangsbelege
bleiben zur Klärung erhalten.

Die einzelne SQLite-Produktprüfung
`crates/storage/tests/android_private_loop.rs` besteht: echte erstellte
Sitzungen, anderer Benutzer abgewiesen, zweifache Modellzulassung abgewiesen,
Dateifreigabe, Lesen, Rückfrage, verschlüsselter Ergebnisdownload,
unveränderte ursprüngliche Erzeugungsquittung und kein erneutes Einreihen einer
unbekannten Absendeoperation. Der Windows-Daemon-/Gerätecheck besteht ebenfalls;
die anschließenden kleinen Zeitfence-/Leerverzeichnisergänzungen werden vom
gemeinsamen aktuellen Quellencheck erfasst. Das ist keine reale Linux-/Telefon-
oder Providerabnahme.

Mit 0053/0055 sind inzwischen eigene private MCP-Bindungen, Streamable HTTP,
Legacy SSE und der kontrollierte stdio-Gastkanal samt Discovery und genau
einmal zugelassenen Original-RPCs geschrieben. Private Prozesse laufen über
einen gesonderten gVisor-Adapter mit unveränderlichem Image-Digest, eigenem
Host-/Engine-Binding und ihrem ursprünglichen Containerjournal. Der Launcher
führt Tenantprozesse als UID/GID 1000 ohne zusätzliche Gruppen aus; ein
getrennter Supervisor verwaltet nur den privaten Gast und dessen Dateibaum.
MCP-Dateibäume werden bei angehaltenen Gastnachfahren erfasst. Jeder Capture
bindet Original-RPC, Originalantwort, Kontroll-ID, Container und Checkpoint;
die verschlüsselten Empfangsbelege bleiben vor einer späteren Veröffentlichung
erhalten. Eine verlorene stdio-Verbindung wird nicht wieder aufgebaut.

Vor Containerentfernung wird der tatsächliche Stop-Beleg dauerhaft gespeichert.
Ein fehlender Container oder eine abgelaufene Lease ersetzt diesen Beleg nicht.
Der abschließende Ergebnissnapshot verlangt geschlossene ursprüngliche
Container und bekannte Effektquittungen. Recovery überspringt aktive Leases
und fremde Runner-Bindings. Binäre Anhänge erhalten beim Loop-Start echte
verschlüsselte Workspace-Dateien; ihr Pfad steht im ausgewählten Kontext.

Actor-Modellreferenzen verwenden das eigene Projekt-/Profilregister und die
gleiche neutrale Provider-Projektion wie der Native-Import. Anlage, aktuelle
Autorität und Originalmodellzulassung prüfen die echte Actor-Bindung. Der
verschlüsselte Modell-Intent bewahrt die vollständige genaue Kontextmessung;
der Dispatch berechnet sie unabhängig erneut. Der gemeinsame Windows-Daemon-
Check 64870 besteht (13,75 s); die Gastdatei besteht die Python-Syntaxprüfung.
Der vorherige Actor-SQLite-Fall besteht (1,00 s), ebenso der KMS-SQLite-Fall
mit dem 0055-Container-Fence (0,18 s). Diese Belege qualifizieren keinen realen
Linux-/gVisor-/MCP-/Telefonlauf. Der neue stdio-Dateiworkflow ist bisher nur
kompiliert, seine physische Produktionsabnahme steht aus.

Für die optionale Linux-Installation liegen
`deploy/v2/android-private-runner.example.json` und
`deploy/v2/compose.android-private.yml` vor. Der Betreiber ersetzt Host-UUID,
den tatsächlich beobachteten Docker-Engine-Identifier und den unveränderlichen
Tools-Image-Digest. Die private JSON-Datei gehört dem Daemon-Benutzer und hat
Dateimodus 0600. API und Worker lesen dieselbe Bindung; ausschließlich der
Worker erhält den ausgewählten Docker-Socket. Der Gast erhält keine Hostmounts.
Das Tools-Image enthält den eigenen Android-Launcher. Eine reale Installation
und Paketabnahme mit diesen Dateien ist noch offen.

Nach diesem Compile sind zusätzlich die elf privaten Office-Operationen
(Text/DOCX/XLSX/PDF/PPTX lesen, Text schreiben, die vier binären Formate
erzeugen und konvertieren) an den eigenen isolierten Programmweg angeschlossen.
Die originale Office-Standardausgabe bleibt im verschlüsselten Prozessbeleg
erhalten. Nicht veränderte Dateien behalten ihre bestehenden Versionen.
Der Betreiber kann Office im privaten Runner explizit freigeben.
Diese Office-Ergänzung ist geschrieben und syntaktisch geprüft; ihr gemeinsamer
Daemonscheck und ihr tatsächlicher LibreOffice-/gVisor-Lauf sind noch offen.

Der private Providerworker veröffentlicht inzwischen beobachtete Textfragmente
als V1-`model_delta` über den privaten SSE-Weg. Die Fragmente liegen in den
vorhandenen verschlüsselten Artefaktversionen; der Eventstrom entschlüsselt sie
mit aktueller eigener Leseberechtigung. Ein gestörter Fortschrittsstrom stoppt
keine bereits zugelassene Originalmodellantwort. Ein unvollständiger Stream
bleibt im Originalbeleg und im Ergebnis als `progress_incomplete` erkennbar.
Auch diese jüngste Streaming-Ergänzung wartet auf den gemeinsamen Quellencheck
und einen tatsächlichen Android-Streamlauf. Die bestehende Loop-Fixture mit
neuem Original-Intentformat besteht (1,10 s); ihr Provider ist synthetisch.
Docker-Capabilitynamen werden mit und ohne `CAP_`-Präfix geprüft, entsprechend
der [Docker-Dokumentation](https://docs.docker.com/engine/containers/run/).

Mit 0060 ist zusätzlich der eigene private Browserpfad geschrieben:
`navigate`, `dom`, `screenshot`, `click`, `fill` und `close` verwenden denselben
ursprünglichen Chromium-Gast innerhalb einer Actor-/Projekt-/Run-/Lease-Bindung.
Jede Aktion benötigt eine exakte Freigabe einschließlich ausgewählter Origins
und HTTP-Methoden. Der gVisor-Gast behält `network=none`; HTTP(S) läuft nur über
seinen ursprünglichen Supervisorkanal und den Daemonbroker. Der Broker prüft
öffentliche DNS-Adressen und pinnt sie für einen einzelnen HTTP80-/HTTPS443-Hop,
verwendet keine Umgebungproxys, automatischen Redirects oder Wiederholungen und
prüft den tatsächlichen Gegenstellen-Socket. Jeder Redirect und jede Unterressource
muss erneut zur freigegebenen Origin gehören. Autorisierungsheader, fremde
Host-/Owner-/Team-Credentials und alternative Netzwerktransporte werden nicht übernommen.
Cookies bleiben an den eigenen Kontext und HTTPS-Zielhost gebunden.
Originale Brokeranforderungen und vollständige beziehungsweise als Teilantwort
gekennzeichnete Empfangsbelege liegen in vorhandenen verschlüsselten Versionen.
Browsersitzungen und einmalige Netzanforderungen sind dauerhaft journalisiert;
verlorene Gäste oder unbekannte Netzwerkeffekte werden nicht wieder aufgenommen.
Browserdateien werden erst nach bestätigtem Stop und Entfernen des ursprünglichen
Containers übernommen, und nur wenn der ursprüngliche Workspace unverändert ist.
Daher muss die Sitzung vor Änderungen durch andere Dateitools geschlossen werden.
Grenzen: vier eigene aktive Kontexte, 16 freigegebene Origins pro Aktion,
512 Anfragen pro Aktion, 2048 pro Sitzung, 512 KiB Anfragetext und 4 MiB
Antwortkörper pro Hop; Originalnetzbelege sind vorab auf 64 MiB pro Sitzung begrenzt.
Python-Gastsyntax und Runner-JSON wurden geprüft. Der gemeinsame Rustcheck und
der physische Chromium-/gVisor-/Android-Nachweis sind noch offen; das Tools-Image
ist noch nicht als Release gebaut oder auf einem echten Betreiberhost qualifiziert.

Offen bleiben die vollständigen privaten GitHub-Tool-APIs,
interaktives Terminal, die Android-Auswahl einer qualifizierten Modellreferenz
in der vorhandenen Oberfläche sowie die weiteren
Android-Kompatibilitätsgruppen (Zeitpläne, Crew, Fork/Version/Merge,
Synchronisationsänderungen, Team-/Auth-/Verwaltung und Cache/Outboxmigration).
Die vollständige Android-Parität ist damit noch nicht abgeschlossen.

Der vollständige Auftrag bleibt aktiv, solange eine Pflichtlieferung offen,
blockiert, unimplementiert oder nur indirekt belegt ist.
