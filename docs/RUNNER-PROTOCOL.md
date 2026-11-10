# Authentifizierte Runner-Zulassung und Produktpfad

Der neue Rust-Dienst `cowork-runner-protocol` prüft signierte Kontrollnachrichten
und speichert ihre Ausführungsidentität dauerhaft in SQLite/PostgreSQL-fähigen
Tabellen. Er benutzt getrennte, explizit gepinnte Controller- und Runner-Schlüssel.
Ed25519-Signaturen besitzen getrennte Zwecke für Launch, Fence, Receipt,
Umgebungsbeobachtung, Ausführungslease und deren Runnerannahme; ein gültiger Beleg ist keine Starterlaubnis für einen
anderen Zweck. Schlüsselrotation oder Runnergeneration ändern keine vorhandene
Installation stillschweigend.

Nach einer ersten atomaren Zulassung besitzt nur der ursprüngliche Aufrufer eine
nicht serialisierbare Starterlaubnis. Identische Wiederholungen lesen den
historischen Beleg. Konflikte um Attempt oder `(run, tool_call)` werden abgewiesen.
Die zusätzliche Dispatch-Transaktion prüft ursprüngliche Frist, Runnerbindung,
Clockhochstand und monotone Lease-Fence erneut und verbraucht die Erlaubnis vor
der OS-Wirkung. Ein Absturz zwischen diesem Commit und tatsächlichem Spawn
bleibt unklar; Neustart erzeugt keine zweite Erlaubnis.

Die genaue Runneridentität bindet Programmauswahl, Argumentvektor, Umgebung,
Arbeitsverzeichnisressource, Workerfence und ursprüngliche Ausführungsfrist.
Das Modell wählt keine Hostpfade, Shellzeichenketten, Environmentvariablen oder
Credentials. Resultate enthalten originale begrenzte Bytes und deren Hashes.
Ergebnisse und Fences sind signiert und werden gegen ihre indexierten SQL-Spalten
geprüft. Supervisorprüfungen lesen genau den unveränderten gespeicherten und
signierten Dispatchauftrag; Admitted, Completed oder veränderte Fristen/IDs
erteilen keine laufende Ausführungsautorität. Eine terminale Beobachtung erweitert keine Launchautorität; ein Entzug
bleibt auch bei Clockrückstellung möglich und öffnet dieselbe Epoch nicht erneut.

Eine laufende Ausführung benötigt zusätzlich eine noch gültige dauerhafte
Controllerlease. Die erste Frist stammt exakt aus dem Launchbeleg. Jede
Erneuerung ist zweckgebunden mit dem Controllerschlüssel signiert und bindet
Runnergeneration/-Build, Attempt, Run, ursprünglichen Launchenvelophash,
Workerhash und dieselbe Epoche. Ihre Revision steigt monoton, ihr Zeitraum
beträgt höchstens 30 Sekunden und endet spätestens an der ursprünglichen
Ausführungsfrist. Der Runner nimmt sie ausschließlich vor Ablauf der bisherigen
Lease für einen tatsächlich dispatchten Auftrag an. Er signiert zusätzlich die
Annahme mit vorheriger Revision/Frist und tatsächlichem Annahmezeitpunkt.
Signaturen und SQL-Indexspalten werden bei jedem Lesen abgeglichen. Eine nur
controllersignierte, nie angenommene Nachricht ist keine gespeicherte
Ausführungsautorität.

Identische erneute Zustellung liest dieselbe Annahme und verlängert nichts.
Andere Nachrichten mit derselben Revision, ältere Revisionen, fremde Bindungen,
künftige Ausgabe, überschrittene Fristen oder eine bereits widerrufene Epoche
werden abgewiesen. Ein beobachteter Ablauf speichert den Clockhochstand; eine
anschließende Clockrückstellung kann den Prozess nicht wieder freischalten.
Ein unabhängiger Adapter überwacht diese Lease auch bei einem hart beendeten
Controller, ohne eine lokale Cancellationnachricht vorauszusetzen.

Der gemeinsame Runtime-Produktpfad nimmt jetzt eine optionale eingefrorene
Umgebungsreferenz bei Einzelagentenstarts auf. Nur mit eigenem schreibbarem
Arbeitsverzeichnis und frisch signierter, vollständig qualifizierter Beobachtung
wird `execute_program` angeboten. Policy/Freigabe, Registry und Lease werden
bei tatsächlicher Zulassung erneut geprüft. Das bestehende Prozessjournal
bestätigt Launch und später Resultat/Checkpoint atomar. Eine verloren gegangene
Bestätigung wird nicht als automatische Wiederholung ausgeführt.

Ein monoton getakteter Monitor verlängert während echter Prozessausführung die
bestehende Storelease und sendet alle drei Sekunden eine daraus abgeleitete,
signierte Ausführungslease an den Runner. Aktueller Runzustand und qualifizierte
Umgebung werden erneut geprüft; die ursprüngliche Laufzeitgrenze bleibt
unverändert. Ist die vorhandene Runnerlease bereits bis zu dieser ursprünglichen
Frist gültig, wird keine weitere Erneuerung erzeugt. Fehler,
Timeouts oder unvollständige/falsch gebundene Antworten signalisieren zuerst lokal
Abbruch, versuchen begrenzt den exakten Fenceentzug und behalten die unklare
Reservierung. Ein fallengelassener Runtimeaufruf propagiert Abbruch ebenfalls.
Der Adapter muss seinen Prozessbesitz unabhängig vom HTTP-/Aufruffuture erhalten,
dauerhafte Fences, die ablaufende Ausführungslease und die originale Frist überwachen und seinen eigenen Baum
physisch beenden/reapen. Erst sein gültiger physischer Ergebnisbeleg darf den
Produktjournal-Eintrag abschließen. Erfolgreiche synthetische Fehler-/Abbruchtests
qualifizieren diesen Lebenszyklusumfang, keine reale Sandbox oder Netzverbindung.

Der private `ProcessRunnerAccess` ist die Transport-/Adaptergrenze. Die
Produktinitialisierung muss dort einen tatsächlich installierten, qualifizierten
Adapter mit gepinnter Identität bereitstellen. Der Dienst akzeptiert keine
Renderer-Capabilitybooleans oder frei behaupteten Reporthashes als Qualifikation.
Bis zur tatsächlichen Windows-/Linux-Sandbox- und Transportabnahme bleibt dieser
Adapter eine offene Lieferung des Gesamtplans. Prozesssupervision, Signaturen
oder erfolgreiche synthetische Tests beweisen allein keine Sandboxisolation.

Die Signaturimplementierung folgt der vorhandenen
[ring-Ed25519-API](https://docs.rs/ring/0.17.14/ring/signature/struct.Ed25519KeyPair.html).
Geteilte Linux-Nodes benötigen weiterhin die ausdrücklich qualifizierte
[gVisor-/OCI-Ausführung](https://gvisor.dev/docs/user_guide/quick_start/oci/).
Keine historische Workflow-/Clock-Abnahme wird auf diese neuen Quellen übertragen.

Die aktuelle abgegrenzte Runtimeprozesssuite besteht unter Windows und Ubuntu
mit zwölf Tests einschließlich zweier reiner Kindprozessfixture-Einstiege.
Die eigentliche harte Crashprobe startet einen separaten OS-Controllerprozess,
bestätigt seine signierte Lease-Erneuerung und beendet exakt diesen eigenen
Prozess über den gehaltenen nativen Childhandle. Der unabhängige Runner stoppt
und erntet seinen echten Kindprozess nach Leaseablauf vor der ursprünglichen
Runfrist. Ein privater neuer gültiger Worker erreicht anschließend den
LaunchCommitted-Guard und erhält `UncertainEffect` ohne zweiten Start.
Dieser Test erzeugt keine öffentliche Resume- oder Reparaturberechtigung.
Eine weitere Probe stoppt den echten Prozess bei verweigerter Lease-Erneuerung;
lange erfolgreiche Ausführungen erneuern ihre Lease ohne Budget-/Friständerung.

Der separate PostgreSQL-Corebeleg (private development evidence; omitted from public sources)
bindet 23 Inputs ohne Drift: 18 SQLitetests, ein tatsächlich ausgeführter
PostgreSQLvertrag mit fünf benannten Prüfgruppen sowie Format und Clippy.
Der eigene Postmaster wurde über seinen gehaltenen Popenhandle mit Exitcode 0
beendet und die private Fixture entfernt. Der
native Windows-Wartebeleg (private development evidence; omitted from public sources)
bestätigt ebenfalls den eigenen WSL-Launcher mit Exitcode 0. Synthetische
terminale Ergebnisse in diesem SQLvertrag beweisen keine physische Ausführung;
die echte Kindprozessprüfung ist der oben getrennte Runtimeumfang. Ein
Produktionsadapter mit qualifizierter Isolation und TLS bleibt erforderlich.
