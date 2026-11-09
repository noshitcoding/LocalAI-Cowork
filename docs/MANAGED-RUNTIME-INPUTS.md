# Bestätigte Eingaben für die eigene Runtime

## Separater HTTPS-Dienst im Aufbau, 31.10.2026

Der optionale Linux-Build `cowork-managed-runtime-service` verbindet jetzt die
native Bootstrap-/Empfangsgrenze mit den gemeinsamen Managed-Wireverträgen.
Er besitzt einen eigenen privaten Broker-Datenbestand für Ausführungsbelege,
keine Projektverwaltung. Seine Konfiguration bindet das tatsächliche Service-ELF,
Runneridentität, TLS, vorhandene Qualifikation, unveränderliche Werkzeugpakete,
begrenztes Datenfilesystem und die eigene cgroup-Generation. Transportauthentisierung
und begrenzte Body-Parallelität erfolgen vor dem JSON-Einlesen.

Transfer/Seal/Start sind vom HTTP-Clientlebenszyklus getrennt. Ein wiederholter
Start mit dem ursprünglichen Ticket liest ausschließlich die Originalquittung.
Alte Launchaufrufe erhalten einen Upgradefehler. Lease-Verlängerungen werden
vorerst ausdrücklich abgewiesen, da der unabhängige physische Wächter weiterhin
die ursprüngliche Frist durchsetzt. Veröffentlichte Datenreservierungen bleiben
gebucht; Neustartübernahme, Freigabeprotokoll und lange Läufe bleiben offen.

Ergebnislesevorgänge benötigen vor und nach den Bytes die frische gemeinsame
Berechtigung. Die ursprüngliche signierte Ergebnisattestation wird einmalig
mit dem Zeitpunkt der Originalquittung gespeichert. Ihre zusätzliche monotone
Aufbewahrungsgrenze stammt vom physischen Abschluss vor Broker-Commit/Export;
CLOCK_BOOTTIME berücksichtigt auch Suspend. Erste UTC-Restfrist und monotone
Fristen können das Fenster nur verkürzen. Keine davon wird beim Lesen erneuert
oder nach einem Dienstneustart aus historischen Bytes rekonstruiert.

Der erste tatsächliche Dienstversuch
`managed-https-service-32084be97a7047dd84fb2950ac84bae1` bleibt fehlgeschlagen:
TLS-Readiness nach 15 Sekunden nicht erreicht, kein Gast gestartet. Seine
211 Quellen waren unverändert und das Archiv vollständig zurückgelesen;
eigene Mounts, Loops, Werkzeuge und Generation wurden entfernt. Testzertifikate
und Fehlerdiagnose wurden anschließend überarbeitet. Die unabhängige Prüfung
fand außerdem die fehlende monotone Output-Retention im ursprünglichen
Dienststand; der gezielte unabhängige ReReview hat die Korrektur am 31.10.2026 um
03:04:14 UTC ohne weitere konkrete Befunde geschlossen. Das ist ein
Quellenreview. Der neue physische Beleg ist im folgenden Absatz gesondert gebunden.
Der neue Dienstversuch
`managed-https-service-10f51e7393b247d9b0e2163f002abe6e` besteht mit 211
festgehaltenen Quellen und ohne Drift. Ein tatsächlicher privater TLS-Dienst
führt zwei gVisor-Gäste aus: bestätigter Snapshot/Kopierlauf mit exaktem
Ergebnisabruf, ursprünglicher dauerhafter Attestation, unveränderten
Wiederholungsbytes und tatsächlichem Retentionsablauf sowie ein Symlink-Erzeuger
mit bestätigtem Exit 0 und separater konkreter Exportablehnung. Jeder Gast hat
zwölf ursprüngliche physische Mutationsquittungen; Startreplay dispatcht nicht
erneut. Zwei gezielte Zeitfälle, native und Service-Clippy sowie Formatprüfungen
bestehen. Eigene Ressourcen wurden vollständig stillgelegt. Die drei erzeugten
ELFs sind unsignierte Entwicklungs-/Prüfarbeitsprodukte. Bindung und Grenzen:
Service-Nachweis (development artifact omitted from public sources).

Der nachfolgende engere Controller-Kompatibilitäts- und Abbruchbeleg
`managed-https-service-f12ede28347241a8bd1fcb560bf2f7ed` besteht ebenfalls über
211 unveränderte Quellen. Er ergänzt die vorhandenen Install-/Observe-Verträge:
Install bestätigt nur die exakt vorhandene, unabhängig qualifizierte Version;
Observe prüft die nativen Pins und gibt deren ursprüngliche Felder frisch
signiert zurück. Unbekannte Pakete/Referenzen und fremde Signaturen scheitern.
Ein dritter tatsächlicher Gast startet nach HTTP-Annahme und bewusst verlorenem
Quittungsinhalt ohne erneuten Startaufruf. Der signierte Widerruf führt zur
physischen Beendigung; Replay liefert die ursprüngliche Canceled-Quittung.
Alle drei dauerhaften Outcomes stimmen exakt mit den signierten Ergebnissen
überein und besitzen zwölf native Mutationsbestätigungen, EOF und bestätigtes
Control-Reaping. Der numerische Exit des abgebrochenen Gasts bleibt ausdrücklich
unbekannt; der rohe interne Waitstatus 128 wird nicht als Exitcode ausgegeben.
Der alte Ergebnislesekontext ist nach Widerruf gesperrt. Echte App-/Controller-
Integration bleibt die nächste gesonderte Abnahme. Die vorherigen Fehlversuche
bleiben mit ihren ursprünglichen Gesamtstatus erhalten.


Ein bestandener Compile-/Clippy-Lauf oder ein einzelner Zeittest schließt diesen
Pfad nicht ab. App-Integration, installierte eigene Distribution, signierte
Pakete und vollständige Plattform-/Gesamtabnahme bleiben offen.

## Tatsächlicher Controlleranschluss in Arbeit

Der nächste private Prüfpfad startet ein exakt SHA-gebundenes Controller-QA-ELF
im eigenen begrenzten Fixture. `prepare` erhält nur die tatsächliche Definition
und öffentliche Runnerdaten. Der produktive Store erzeugt Projekt, Environment,
Version sowie die regulär registrierte und aktivierte Zielreferenz selbst;
der native Dienst übernimmt genau diese IDs/Revision, ohne SQL-Injektion.
`exercise` erhält einen privaten 0600-Konfigurationspfad und verwendet den
unveränderten produktiven HTTPS-Loader. Die Controllerautorität bleibt lokal.

Der Testprogrammmodus `owner_copy` liest und schreibt unter dem vom bestätigten
Plan gewählten Kopieralias. Ein erfolgreicher Callback muss eine dauerhafte
Ergebnisversion melden; der native Prüfer bindet dessen Manifest und Quittung
zusätzlich an die tatsächliche Serviceantwort und den dauerhaften Prozessbeleg.
Controller- und native Quellabschlüsse werden vor dem Lauf zusammengeführt und
archiviert. Der Hook und das Testprogramm kompilieren; ein erfolgreicher
Controllerlauf ist damit noch nicht nachgewiesen. Die Drei-Gast-Dienstabnahme
oben bleibt ein getrennt archivierter Quellsnapshot. Keine App-/UI-/Installations-
oder Plattformfreigabe wird daraus abgeleitet.

Die gekoppelte Diagnose `managed-https-controller-1e2fd6bad6fe47e6a4cf6c6cd2102f80`
belegt einen Konkurrenzfehler: Eine Environment-Beobachtung trifft während der
eigenen Volume-Reservierung auf deren nativen Pool-Lock. Der Controller widerruft
den Auftrag daraufhin; kein Gast startet. Alle 901 Quellen und das Archiv bleiben
unverändert, die eigenen Ressourcen sind entfernt.

Der ursprüngliche Pool-`Arc` synchronisiert seine Transaktionen jetzt lokal,
bevor der unveränderte native Besitzer-Lock geprüft wird. Separat geöffnete Pools
bleiben ausgeschlossen; vergiftete lokale Locks scheitern geschlossen. Der
gezielte ext4-Regressionsfall scheitert vor der Korrektur (`a253c1…`) und besteht
danach (`b3ab64…`), einschließlich ursprünglicher Receiver-, Ersatz-, Neustart-
und Reservierungsprüfungen. Service-Clippy und unabhängiger Quellenreview
bestehen. Der nachfolgende gekoppelte Lauf (`e32100…`, 904 unveränderte Quellen)
erreicht ohne Sperrkollision die Missing-Chunk-Abfrage, scheitert dort aber vor
dem Gaststart. Die Ursache dieser Ablehnung bleibt offen. Quellen, Originalfehlschläge
und Grenzen sind im Controller-Nachweis (development artifact omitted from public sources)
gebunden.

Der anschließende Lauf `managed-https-controller-3e6e7026645d4bc19c957036d813f4da`
besteht mit 904 unveränderten, vollständig rückgelesenen Quellen. Der produktive
Controller bestätigt den Snapshot, überträgt ihn über den produktiven TLS-Client,
führt ein Programm im tatsächlichen gVisor-Gast aus und speichert dessen Ergebnis
nach Prüfung als lokale Ready-Version. Die ursprüngliche signierte Quittung,
Manifest- und Chunkbytes sowie Wiederholungsprüfungen stimmen überein. Der Gast
hat bestätigten Exit 0, zwölf native Mutationsquittungen, EOF und bestätigtes
Control-Reaping. Eigene Mounts, Loops, Werkzeuge und Generation sind entfernt;
native und Service-Clippy sowie Formatprüfungen bestehen.

Zwischen der früheren Missing-Chunk-Ablehnung und diesem Erfolg wurde nur der
Diagnosekontext erweitert. Der frühere Fehler gilt deshalb weiterhin als
ungeklärt. Dieser eine bestandene Controllerpfad qualifiziert weder Stabilität,
Windows-Oberfläche, installierte Distribution noch die vollständige Plattform.

Am 01.11.2026 besteht zusätzlich der tatsächliche Owner-Abbruchpfad
`managed-https-controller-927ee93b58244f28a4ce34b5cc0af303` über 910 unveränderte
Quellen. Erst originale native Startquittung und Gastmarker erlauben dem
Controller-Test, den regulären `Store.cancel` aufzurufen. Nach Interrupted ohne
Lease wartet er auf die ursprüngliche signierte Abschlussquittung und gleicht
sie über den produktiven Owner-Adapter ab. Der Auftrag wird fachlich Canceled;
der ursprüngliche Prozessversuch speichert separat ReconciledCompleted mit
Canceled-Ergebnis. Exakter Replay ändert weder Ereignisse noch Prozessanzahl und
startet weder Programm noch Modell erneut. Eine Ergebnisversion wird nicht
implizit übernommen; spätere Remote-Leserechte bleiben ein eigener offener Pfad.

Start- und Abschlussrecord sind als Originalbytes gesichert. Ihre Bytehashes
und der kanonische Recordhash der nativen Recovery werden getrennt geprüft.
Zwölf native Quittungen, physischer Stop, EOF und Control-Reaping sind belegt.
Der rohe Waitstatus 128 ist kein bestätigter numerischer Gast-Exit und bleibt im
signierten Ergebnis unbekannt. Alle eigenen Testressourcen sind entfernt;
Clippy und Formatprüfungen bestehen. Die Stilllegung dieser privaten
Testgeneration ist keine Freigabe produktiver Datenreservierungen und keine
vollständige Plattformabnahme.

## Aktueller Anschluss und Sicherheitskorrektur vom 30.10.2026

Der gemeinsame Typ `ManagedInputGrant` liegt jetzt in `runner-protocol`; der
bisherige native Importpfad reexportiert dieselben signierten Bytes. Die App-/
Store-Seite stellt ihn aus dem tatsächlich bestätigten ManagedRunner-Plan und
seiner überprüften Arbeitskopie aus. Die neue native `IncomingManagedInput`-
Grenze verlangt pro Operation einen frischen, einmaligen Input-Permit des
gemeinsamen Brokers. Transfer und vollständige Materialisierung erfolgen vor
der eigentlichen Ausführungszulassung. Der HTTP-Anschluss bleibt offen.

Eine Transferoperation erzeugt keine Execution-Quittung. Ein eigener
runner-signierter Custody-Marker im bestehenden Broker hält jedoch fest, dass
dieser Originalversuch bereits eine erste Input-Epoche erhalten hat. Nur die
ursprüngliche laufende Brokerinstanz hält deren monotone Frist. Neue Permits
verlängern sie nicht. Eine neue Instanz verweigert denselben Versuch auch mit
neuem Volume; sie rekonstruiert niemals einen Instant aus Wanduhrdaten. Der
Marker wird erst atomar mit einer dauerhaften Completed-Quittung entfernt.
Historischer Replay liefert weiterhin ausschließlich die Originalquittung.

Rootprivate native Custody bindet zusätzlich Launch, signierten Grant, Manifest,
Volume und ursprüngliche Brokerinstanz. Reopen, Seal und Task-Vorbereitung
behalten diese Instanz und die erste monotone Frist. `bind_received` ist nur
noch containerd-intern erreichbar. Eine andere Datenbank mit identischen
Schlüsseln kann keine bereits empfangene oder versiegelte Eingabe übernehmen.
Die unabhängige Nachprüfung hat beide gefundenen Instanz-/Fristumgehungen
einschließlich Neustart/neuem Volume nach der Korrektur geschlossen.

Windows und Linux bestehen mit den gemeinsamen Wiretypen jeweils 36 Protokolltests; der externe PostgreSQL-
Fall wurde dabei ausdrücklich nicht ausgeführt. Der native Incoming-Test
`managed-native-input-70ac69f7999f458f8712c730a1d12590` bleibt **fehlgeschlagen**:
Transfer, Chunkreplay, Original-Reopen, Fremdbroker-Ablehnung bis nach Seal und
native Baumsubstitution wurden beobachtet; der dritte Fall wurde nach 1,16 s
durch tatsächlichen `ClockRollback` gestoppt. 183 Eingabequellen wurden vor dem
Build archiviert, vollständig zurückgelesen und nach dem Fehler ohne Drift
verglichen. Kein Gastprozess startete; eigene Mounts, Loops und Generation
wurden vollständig entfernt. Dieser Teilbefund ist keine aktuelle Gesamt-
oder Plattformfreigabe. Der alte Rohbinder-Komponentenfall ist im neuen
Incoming-Pfad aufgegangen; die separate Komponentensuite zählt jetzt fünf
physische Fälle. Historische sechs-Fälle-Berichte bleiben unverändert.

Der additive `containerd/backend.rs` verbindet diese Fähigkeiten bereits mit
Ressourcenreservierung, gemeinsamem Admit/Dispatch, Task und Abschlussquittung.
Sein Worker überlebt das Verwerfen des Clienthandles; ein ungeklärter Effekt
sperrt den einzelnen Rechenplatz. HTTP, App-Ergebnisübernahme, Freigabe
veröffentlichter Volumes und physischer Ende-zu-Ende-Nachweis bleiben offen.
Schreibgeschützte Aliasse werden auf Controllerseite bis zur zusätzlichen
portablen Rechtebindung und nativen RO-Mount-Durchsetzung verweigert.

Der Exportstatus ist inzwischen vom physischen Abschluss getrennt: Nach
`ManagedTask.run` wird zuerst die ursprüngliche signierte Completed-Quittung
dauerhaft im gemeinsamen Broker abgeschlossen. `ManagedOutputStatus` meldet
anschließend entweder einen überprüften Snapshot oder eine konkrete
Exportablehnung. Eine Ablehnung erzeugt keine Snapshot-Fähigkeit oder
Ergebnisattestation und löst keinen erneuten Gastlauf aus. Der vorab reservierte
8-MiB-Slot speichert im lokalen Format 2 den Status `ready`/`rejected` zusammen
mit dem ursprünglichen Receipt-SHA und dem physischen Beleg; ein gescheiterter
Persistenzversuch wird ausdrücklich ohne behaupteten Record-SHA zurückgegeben.

Die beiden tatsächlichen F47-Proben
`managed-native-backend-rejected-4619e4d7ec144b5cab34f70f8e2b6d40` und
`managed-native-backend-e51cb15d02264815867c7e3257359aaf` bestehen mit
190 gebundenen Quellen: echter Symlink-Erzeuger mit Exit 0, separater dauerhafter
Exportablehnung, unveränderlicher Completed-Historie und ohne Redispatch sowie
der positive Kopier-/Ergebnisweg. Beide Sourcefenster bleiben historische
Snapshots; spätere Controller-Brokeränderungen werden dadurch nicht qualifiziert.
Eigene Mounts, Loops und Generationen sind entfernt. Die unabhängige
F47-Nachprüfung ist am 30.10.2026 um 17:35:31 UTC ohne weitere Befunde geschlossen.

Der zusätzliche physische Eigentümerschaftstest
`managed-receiver-claim-e7e136d03d38467489c8ec048392d3ad` besteht: Ein zweiter
aktiver Empfänger und ein ersetzter nativer Lock werden abgewiesen. Ein neuer
Empfänger über einem Pool mit behaltenen Reservierungen wird gesperrt. Erst
nach tatsächlichem Retirement einer nachweislich unveröffentlichten Reservierung
ist ein neuer Claim möglich; veröffentlichte 32 MiB bleiben gebucht. 185 stabile
Quellen und das tatsächliche Release-Test-ELF sind gebunden; eigene Fixture,
Mounts und Loops wurden entfernt. Dieser Test startet keinen Gast und prüft
weder die Uhr noch den vollständigen Backendablauf. Der Backendstand besteht
den Linux-Feature-Clippy für alle Targets. Gebundener Teilnachweis und offene
Abnahme (development artifact omitted from public sources).

## Tatsächlicher nativer Backendpfad und gemeinsame Wiretypen

Der getrennte Snapshot
`managed-native-backend-b43174bd0adc489a90dfecc0d27c3eb5` besteht mit einem
echten Kopiergast durch `ManagedReceiver`: nicht reservierende Bereitschaft,
ursprünglicher Transfer mit Blockreplay und Wiederaufnahme, vollständige
Eingangsprüfung vor Execution-Admission, zwölf genaue Root-/Task-ACKs,
bestätigter Gaststatus, Kontrollprozess-Reap, Ausgabe-EOF und native Stilllegung.
Ergebnismanifest und Blöcke stimmen; ein mit identischen Bytes ersetztes natives
Ergebnis wird abgewiesen. Der vor Programmstart tatsächlich vorallokierte
8-MiB-Wiederherstellungsslot wurde einschließlich SHA und Inhalt zurückgelesen.
Das Profil besitzt 16.384 Inodes; die portable Scanobergrenze wird getrennt
auf maximal 10.000 Einträge begrenzt. Die 8-MiB-Reserve deckt zwei Manifestbudgets,
10.001 begrenzte Native-Stamps und 64 KiB Grant-/Belegrahmen ab. Eine akzeptierte
8.001-Einträge-Regression überschreitet die alte 4-MiB-Grenze und passt in die
neue Reserve.

188 stabile Quellen, reale Release-ELFs, Feature-Clippy und Format sind gebunden;
Source-Archiv-SHA256:
`dc49287335bda446f89bd61355d6cc58de2dac5a9cb872e6b486415bf051be65`.
Eigene Fixture, Mounts, Loops und Generation wurden entfernt. Der frühere
`managed-native-backend-6529ad4aa6e44184926034508153ba1c` bleibt ein eigener
186-Quellen-Snapshot mit 2.048 Inodes/4-MiB-Reserve. Beide verwenden synthetische
signierte Plan-/Qualifikationsmetadaten und qualifizieren weder HTTP-/Apppfad
noch Plattform, Paketinstallation oder den vollständigen Isolations-/Lastumfang.

`runner-protocol/managed_transport.rs` definiert die gemeinsamen privaten
Format-1-Verträge: signierte noncegebundene Bereitschaft (höchstens 15 s),
signiertes Ticket mit ursprünglichem Launch-/Grant-/Manifest-/Readinessbezug,
begrenzt eindeutige fehlende Blöcke, Blockquittung, Start und Originalhistorie.
Davon getrennt sind die signierte unveränderliche Ergebnisattestation mit
höchstens sieben Tagen Aufbewahrung und ein frisch durch den Controller
autorisiertes, höchstens 30 s gültiges Leserecht pro konkreter Ergebnisoperation.
Ein historischer Execution-Beleg ist kein Inhaltsleserecht; eine abgelaufene
Workerlease wird nicht erneuert. Die Controller-HTTPS-Implementierung entsteht
im getrennt koordinierten Daemon-Untermodul; der native HTTP-Empfänger ist noch
anzubinden.

## Historischer Bausteinnachweis

Der private Empfängerpfad verbindet jetzt einen signierten Auftragsbezug mit dem
vorhandenen WorkspaceReceiver, einem tatsächlich reservierten ext4-Volume und der
einmaligen Dispatchübergabe des gemeinsamen Brokers. Die Komponenten sind
implementiert und physisch geprüft. Die App stellt diese Freigabe noch nicht aus;
Transfer-Endpunkte bleiben offen. Tatsächliche containerd/gVisor-Aufträge sind
inzwischen als eigener [Taskbaustein](MANAGED-RUNTIME-TASKS.md) implementiert und
mit einem getrennten Quellen-/Artefaktnachweis geprüft. Der ältere Komponenten-
Nachweis dieses Dokuments bleibt ausschließlich an seine ursprünglichen Bytes
gebunden und erteilt keine Produkt- oder Plattformfreigabe.

## Freigabe und tatsächlicher Empfänger

`containerd/input.rs` definiert `ManagedInputGrant` und die nicht serialisierbaren,
nicht klonbaren Fähigkeiten `VerifiedInputGrant` und `VerifiedTaskInput`.
Der gemeinsame Runner-Signierer verwendet dafür den getrennten Zweck
`managed_input_grant`. Vorhandene Zwecke und Signaturbytes bleiben erhalten.

Der Grant bindet bestätigte Plan-ID und Plan-SHA, den portablen Eingabemanifest-SHA,
die tatsächliche `ExecutionTargetRef` und die vollständige `ExecutionRunnerIdentity`.
Zielrevision und Empfängergeneration werden getrennt verglichen. Zusätzlich sind
Run, Prozessversuch, Request-SHA, konkrete Invocation, genaue Werkzeugumgebung,
Worker-Fence und Leaseepoch gebunden. Die Gültigkeit beträgt höchstens fünf Minuten.
Ein Hash oder synchronisierte Projektbytes allein erzeugen keine Berechtigung.

Der vertrauenswürdige Aufrufer muss den aktuellen Zielbeleg und seine aktuelle
Autorisierungszeit unabhängig auflösen. Der Grant darf erst nach Bestätigung des
Plans und gemeinsamer aktueller Zulassung ausgestellt werden. Seine Signatur ersetzt
keine Rollenprüfung, keinen Widerruf und keine laufende Ausführungslease. Die
App-Ausstellung und diese frischen Prüfungen an den künftigen Übertragungsendpunkten
sind noch anzubinden. Es entsteht keine zweite Projekt-/Berechtigungsverwaltung.

## Vollständiger Snapshot und lokale Dateigrenze

`transfer_binding()` liefert den vorhandenen Receiver-Vertrag mit Planbindung und
tatsächlichem Empfänger. Der bestehende Receiver überprüft alle Blöcke und Dateien
und liefert erst danach seine `ReceivedWorkspace`-Fähigkeit. Eine beliebige
Verzeichnisangabe kann diese Fähigkeit nicht ersetzen.

`bind_received()` akzeptiert ausschließlich diesen verifizierten Baum im exakt
passenden veröffentlichten Volume. Neben Manifest und Request müssen die nativen
Kennungen der tatsächlichen `work/workspace`-Unterwurzel übereinstimmen. Identische
Inhalte in einem anderen Verzeichnis werden abgewiesen. Native Device-/Inode-Werte
bleiben lokale Merkmale und erscheinen nicht im portablen Grant.

Nur Dateien und Verzeichnisse der überprüften Arbeitskopie erhalten UID/GID 1000
und private Datenrechte. Die getrennte temporäre Unterwurzel muss leer sein.
Transferstaging, Volumejournal und Kontrollwurzel behalten ihre Root-Rechte und
werden nicht an den Workload gemountet. Die bisherige Image-/Mount-/Loop-Prüfung
bleibt aktiv. Fehler und verlorene Übergaben geben veröffentlichte Reservierungen
nicht frei.

`validate_dispatch()` verlangt die echte `DispatchedLaunch`-Fähigkeit des gemeinsamen
Brokers. Kopierte Requestbytes und gespeicherte Quittungen reichen nicht. Die
Bestätigung prüft die konkreten Auftragsfelder erneut: Derselbe Request-SHA als
bloßes Label kann weder eine andere Invocation noch eine andere Umgebung zulassen.
Auch ein früherer gültiger Zielbeleg ersetzt keine frische Registryprüfung.

## Eigener Pakettyp und geschützte Programmressourcen

Die private `EnvironmentDefinition` kennt jetzt Format 3 mit
`ManagedLinuxPackage`. Es bindet Paket-, Rootfs- und gVisor-Manifesthash sowie
Datei-/Ausgabegrenzen. Derzeit ist ausschließlich `NetworkDenied` zulässig. Alte
Docker-/Kata-Umgebungen und ihre Qualifikationen werden nicht in dieses Profil
umgedeutet. Format-1/2-Serialisierung bleibt erhalten.

`gvisor.rs` überprüft das vollständige unveränderliche Werkzeugpaket einschließlich
der Hilfsprogramme. `rootfs.rs` überprüft alle Dateien und Links eines versiegelten
Paketbaums. Programmpfade werden anhand des Paketmanifests mit Gastwurzel-Semantik
aufgelöst; absolute Links werden nicht auf dem Host verfolgt. Fremde, zusätzliche,
schreibbare oder ersetzte Paketbytes sperren die Bindung. Die aktuell verwendete
[gVisor-Version](https://github.com/google/gvisor/releases/tag/release-20261026.0)
und ihre Prüfsummen sind fest gebunden. Das moderne
[vollständige Paket](https://gvisor.dev/docs/user_guide/install/) enthält auch
die von `runsc` benötigten Hilfsprogramme.

`cgroups.rs` erstellt ausschließlich neue eigene cgroup-v2-Kinder und setzt CPU,
RAM, Swap, OOM-Verhalten und Prozessgrenze. Vorhandene Host-Controller werden nur
gelesen. Native Identität und tatsächliche Grenzwerte werden erneut geprüft.
Dieser Konfigurationsnachweis ist noch kein Belastungsnachweis für gVisor-Tasks.

`output.rs` verwendet private FIFOs und begrenzt den behaltenen Speicher pro Stream.
Beim Erkennen eines überschrittenen gemeinsamen Ausgabebudgets setzt es Rückstau;
Block-/Pipepuffer sind dabei ausdrücklich zusätzlich begrenzt. Der künftige
Task-Supervisor muss daraufhin den eigenen Task beenden. Die Endausgabe verlangt
beobachtetes EOF; ein Drain-Timeout wird nicht zu vollständiger Ausgabe erklärt.
Originalbytes, Gesamtzählung, Abschneiden und ungültiges UTF-8 bleiben erkennbar.

## Aktueller Nachweis und nächster integrierter Pfad

Der gebundene Entwicklungsnachweis (development artifact omitted from public sources)
enthält 193 stabile Quelleninputs, den tatsächlichen Linux-Prüfcode und seine
Prüfprogramme. Sechs physische Komponentenfälle prüfen die echte Empfänger-/Volume-
Verbindung, UID-Konvertierung, einen fremden nativen Gegenbeleg, exakte gemeinsame
Dispatchbindung, FIFO-Rückstau, Paketbytes und eigene cgroup-Konfiguration.
Eine Signaturmatrix sowie je sechs Environment- und 21 Brokerfälle unter Windows
und WSL bestehen. Feature-Clippy des eigenen Pakets und Formatprüfung bestehen.

Die erneuten 13 Volume-Regressionsfälle mit Paketprüfer-/Builderregressionen sind
als eigener Bericht auf überlappenden identischen Quellen beigefügt. Eigene Mounts,
Loop-Geräte, cgroups, Datenfixture und öffentliche Kopie der Testwerkzeuge sind
entfernt. Die ursprünglichen Nachweise bleiben unverändert.

```powershell
python tools/managed_runtime/probe_task_components.py --wsl-distro Ubuntu-24.04
```

Die Tests verwenden eigene Schlüssel und synthetische Planmetadaten. Sie ersetzen
keinen tatsächlichen App-Plan-/Transfer-/Startablauf. Der getrennte Tasknachweis
deckt inzwischen tatsächliche Starts, unabhängige Abbruch-/Fristgrenzen und
getrennte Task-/Sandbox-/Kontrollprozessbelege ab. App-Minting, Transferendpunkte,
Ergebnisübernahme, Server-/Gerätepfade und Plattformqualifikation müssen diesen
Pfad weiterhin integrieren und vollständig prüfen.
Ein leerer cgroup oder beendeter Daemon allein gibt keine Ressource frei. Paket C,
der gesamte A–I-Umbau und die Releaseabnahme bleiben offen.
