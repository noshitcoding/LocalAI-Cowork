# Bestätigter Owner-Plan und Eingaberecht für die eigene Runtime

Der Owner-Controller kann jetzt einen ausdrücklich ausgewählten registrierten
Runner im gemeinsamen PlanV2 binden und aus einem tatsächlich bestätigten Lauf
ein enges Eingaberecht ausstellen. Die Implementierung verwendet den vorhandenen
Run-, Prozessversuchs-, Workerlease- und Workspace-Custody-Pfad. Sie erstellt
keine zweite Laufverwaltung. Die vollständige App-/Runtime-Dienstanbindung,
Team-/Gerätepfade und Releaseabnahme bleiben im Gesamtauftrag offen.

## Plan und frische Zulassung

`PrepareOwnerExecutionPlanV2.target` kennt zusätzlich `registered_runner`.
Der Client übergibt damit keine Runner-Identität oder Qualifikation. Der
Controller löst die konfigurierte, ausdrücklich aktivierte Registrierung selbst
auf. Der unveränderliche `managed_runner`-Plan enthält ihre genaue Revision,
vollständige öffentliche Runner-Identität, die Programme der qualifizierten
Umgebung und den weiterhin benötigten nativen Kopie-Controller.

Vorhandene `broker_controller`-Planbytes behalten ihre Bedeutung. Dieser Pfad
bleibt `BrokerOnly` ohne Prozessumgebung. Der neue Resolver verlangt ein
qualifiziertes `ManagedLinuxPackage`, gesperrtes Prozessnetzwerk und den separaten
Transportpfad. `managed_input_ready(environment, target)` muss die aktuelle
Bereitschaft des installierten Profils authentifiziert bestätigen; die
Standardimplementierung älterer Transporte lehnt ab. Registrierung und Umgebung
werden beim Commit und vor neuen Effekten erneut geprüft.

Der erste Controllerbaustein unterstützt Single-Owner-Aufträge. Workflowprogramme
benötigen noch die Bindung an ihr tatsächlich gespeichertes Workflow-Tooljournal.
Ordner mit Leserechten werden für dieses neue Prozessziel abgewiesen, solange
der Empfänger die jeweiligen Aliasrechte nicht portabel gebunden und nativ
durchgesetzt hat. Ihre vorhandenen Brokerabläufe bleiben verfügbar. Diese offenen
Teile sind keine Verkleinerung des beauftragten Gesamtumfangs.

## Ein ursprünglicher Prozessversuch und eine genaue Kopie

Erst die bestehende einmalige Prozesszulassung erzeugt ihren nicht
rekonstruierbaren `ProcessLaunchPermit`. Der Controller bindet den anschließend
signierten ursprünglichen `LaunchRequest` an diesen gespeicherten Versuch und
den aktuellen Worker-Fence. Ein gültig signiertes Workspace-Label allein genügt
nicht: Planquittung, Actor, RunSpec, Kopie-Custody, Invocation, Zielrevision,
Umgebung und das aktuelle Laufrecht müssen zusammenpassen.

Der Broker prüft die ursprünglichen Ressourcenrechte und die native Identität
der bereits bestätigten Arbeitskopie. Er friert genau deren aktuellen Inhalt als
abgeleiteten Snapshot ein. Kopie- und Ressourcenlocks bleiben bis zur dauerhaften
SQL-Custody erhalten. Änderungen an Originalen werden nicht als Ersatzquelle
verwendet. Migration 113 speichert den privaten Transferbeleg beim vorhandenen
Prozessversuch und dem vorhandenen bestätigten Plan.

`ManagedInputGrant` stammt aus dem gemeinsamen privaten Runner-Protokoll. Er
bindet Plan-ID/SHA, Ziel und Runner, Manifest, ursprünglichen Run und Versuch,
Request, Invocation, Umgebung, Worker-Fence und Lease-Epoche. Seine Gültigkeit
überschreitet im Controller nicht die anfängliche Launchfreigabe von höchstens
30 Sekunden. Eine längere Workerlease oder ein späterer Heartbeat verlängert
dieses Eingaberecht nicht. Physische Lease-Erneuerung bleibt separat zu lösen.

## Übertragung und Wiederholung

`ProcessRunnerAccess::managed_launch` erhält die signierte Anfrage und eine
private `ManagedInputAccess`-Fähigkeit. Der Standard lehnt ab. Der Controller
verwendet bei einer gebundenen Arbeitskopie keinen Legacy-Launch als Ersatz.

Die Quelle liefert nur den signierten Grant, das portable Manifest und genau
erlaubte, verifizierte Dateiblöcke. Zugangstoken, native Kopiebelege,
Original-FolderGrants und Hostpfade werden nicht übertragen. Jeder Abruf prüft
erneut Workerzustand, Frist, ursprünglichen Versuch, Zielrevision, Umgebung,
Kopie-Custody und Ressourcenrechte. Blockpfad, Offset und SHA müssen im
gespeicherten Manifest stehen. Fehlende oder beschädigte Retained-Bytes führen
zu einem Fehler; Original und veränderliche Kopie sind keine Ausweichquelle.

Eine identische Wiederholung erhält den ursprünglichen Transferbeleg. Geänderte
Bytes oder Bindungen überschreiben ihn nicht. Weder Lesen noch Wiederöffnen
erzeugen einen weiteren LaunchPermit. Unklare Wirkungen bleiben im bestehenden
Journal; der native Empfänger muss vor dem einmaligen gemeinsamen Dispatch die
vollständige Übertragung verifizieren.

Scheitert die lokale Vorbereitung nach der Prozesszulassung, aber vor der
Transportübergabe, verbraucht der Controller seine noch vorhandene ursprüngliche
Startberechtigung für einen dauerhaften Nichtstartbeleg. Der Store bindet dabei
den ursprünglichen Store, Versuch und Worker; gespeicherte Versuche und Replays
können diesen Beleg nicht erzeugen. Die Transaktion schließt das Werkzeugjournal
und bei einem Benutzerabbruch auch den Lauf. Nach erfolgter Transportübergabe
bleiben unklare Ergebnisse weiterhin unklar und werden nicht so abgeschlossen.

## Prüfung und verbleibende Integration

Neun SQLite-Fälle prüfen die tatsächlichen Store-/Brokerpfade mit eigens
erstellten Fixture-Schlüsseln und ausdrücklich synthetischer Qualifikation:
abgeleitete Kopie und Replay, geänderte signierte Intentfelder, Zielreaktivierung
und Abbruch, beschädigte Blöcke, Ressourcenwiderruf, frische Commitprüfung und
fehlende Unterstützung eines alten Transports und den ursprünglichen
Nichtstartabschluss mit anschließend gesperrten Eingabeabrufen. Die neun PostgreSQL-Fälle verwenden
denselben eingefrorenen Testcode auf je einer eigenen leeren Datenbank mit Schema
113. Alle neun bestehen; ihre Datenbanken sind anschließend nachweislich entfernt.
Der gebundene Bericht liegt unter
`.local/remediation-20261024/managed-owner-inputs/postgres-qualification.json`.
Der begrenzte Gesamtbeleg dieses Controllerbausteins (development artifact omitted from public sources)
enthält zusätzlich 26 Prozess-, 23 Snapshot- und 590 Frontendregressionen,
Vertragskonsistenz und Typ-/Webbuild. Er ersetzt keine Dienst- oder Plattformabnahme.

Der SDK prüft die neue Zielidentität, Programm- und Netzwerkgrenze sowie die
Bestätigungsquittung. Die Plananzeige zeigt Empfänger, Programme und die anfängliche
Ausführungsgrenze. Das ist noch kein tatsächlicher App-zu-Runtime-Programmlauf.
Die Startprüfung liest nun zuerst die genaue gewählte aktive Umgebung. Für ein
eigenes Runtimepaket verlangt sie die sichtbare Auswahl der registrierten Runtime
vor der Planprüfung. Geänderte und widerrufene Versionen sowie ungeeignete
Rechtekombinationen wählen keinen Ersatzpfad. Gespeicherte ursprüngliche Pläne
und bereits bestätigte ältere Quittungen behalten nach Reload Vorrang.
Dieser neue Einstieg ist mit 13 gezielten UI-Fällen und Typprüfung geprüft;
ein tatsächlicher Browser-/Programmlauf bleibt offen.

Die eigene lokale Entwicklungs-App wurde mit dem eingefrorenen neuen Daemon und
Webartefakt gestartet. Das bestehende Testprofil ist auf Schema 113 aktualisiert;
alle 398 vorherigen Tabellendefinitionen, Daten und Migrationsbelege bleiben
erhalten. Verbindung, vorhandene Projekte und die Umgebungsverwaltung wurden im
Browser geprüft. Dieser Oberflächencheck führt noch kein Managed-Programm aus.
Die getrennten nativen Befunde zur ursprünglichen Brokerinstanz, monotonen Frist
und zum Neustart sind im statischen Nachreview geschlossen. Die tatsächliche
physische Laufzeitqualifikation und der installierte Dienst bleiben offen.

Offen bleiben der authentifizierte installierte Dienst/Transport und dessen
End-to-End-Test mit tatsächlicher Zielauswahl, Workflow- und Aliasrechte,
Ergebnisübernahme/Volume-Freigabe sowie die vollständige Plattformqualifikation.

Der Controllerteil des HTTPS-Transports ist inzwischen implementiert. Er prüft
signierte Bereitschaftsbelege mit frischer Requestkennung, vollständige
Launch-/Grant-/Manifest-/Instanzbindungen im Transferticket und jeden angeforderten
Block. Vor dem einmaligen Start fragt er die tatsächliche Inhaltsberechtigung
erneut ab. Verlorene Begin-/Startantworten bleiben unklar; es erfolgt kein zweites
Start-POST. Abbruch beendet auch wartende Antwortlesevorgänge. Die ursprüngliche
monotone Frist wird nicht neu verankert. Neun gezielte Fälle bestehen mit einem
synthetischen signierten Peer, davon fünf Testfunktionen über eine tatsächliche
lokale TLS-Verbindung. Unabhängiger statischer Review und alle 809 Quellinputs
des Testbuilds sind gebunden. Dies ersetzt noch keine Verbindung mit dem nativen
Dienst oder einen tatsächlichen App-zu-Managed-Programmlauf. Der getrennte Beleg
liegt unter `.local/remediation-20261024/managed-owner-inputs/https-managed-client-qualification.json`.

Die ursprüngliche signierte Abschlussquittung wird jetzt vor dem bestehenden
Prozessabschluss dauerhaft aufbewahrt. Migration 114 bindet diese beobachtende
Historie an denselben Eingabetransfer, Plan und Prozessversuch. Lesen prüft
Signaturen, Envelope, Tabellenindizes, Ergebnis und Worker-Fence erneut. Eine
abweichende Quittung überschreibt das Original nicht; nachgewiesener Nichtstart
wird nicht nachträglich in einen abgeschlossenen Lauf umgedeutet. Das Speichern
erteilt keine Inhaltsrechte und belebt weder Workerlease noch Ausführung wieder.

Der neue getrennte Prüfstand umfasst neun Eingabe- und sieben Quittungsfälle:
16 auf SQLite und dieselben 16 mit demselben eingefrorenen Testprogramm auf je
einer eigenen PostgreSQL-Datenbank. Alle Prüfdatenbanken sind entfernt. Hinzu
kommen 26 Prozess- und sechs Migrationsregressionen. Die tatsächliche Migration
einer Kopie des vorhandenen Profils von 113 auf 114 erhält alle 399 bisherigen
Tabellen und alten Migrationsbelege. Das laufende Entwicklungsprofil bleibt
auf 113. Quellbindungen und Grenzen stehen im Feld `original_completion_history`
des Validierungsbelegs (development artifact omitted from public sources). Der neuere getrennte
Baustein für [frische Ergebnisleserechte](MANAGED-RESULT-READS.md) ist inzwischen
implementiert: 27 SQLite-/27 isolierte PostgreSQL-Fälle mit identischem Testprogramm
und 14 HTTPS-Clientfälle einschließlich tatsächlicher lokaler TLS-Verbindung
bestehen. Alle neuen Prüfdatenbanken sind entfernt. Er prüft aktuelle Rechte sowie
die ursprünglichen Input-, Launch-, Abschluss- und Outputbindungen vor und nach
dem Abruf. Die unabhängige Nachprüfung fand eine während erneuter Autorisierung
ablaufende Aufbewahrungsfrist; erneute Attestationprüfung und monotone Restfrist
schließen diesen durch einen tatsächlichen Verzögerungsfall geprüften Pfad.
Der separate Beleg (development artifact omitted from public sources) ersetzt keine native
Dienstanbindung, Ergebnisversion mit Übernahmeprüfung oder App-Ende-zu-Ende-Abnahme.

```powershell
cargo test --locked -p cowork-runtime --test managed_inputs
```

Ergänzende Grenzen und physische Entwicklungsnachweise:
[bestätigte Eingaben](MANAGED-RUNTIME-INPUTS.md) und
[eigener gVisor-Taskpfad](MANAGED-RUNTIME-TASKS.md).
