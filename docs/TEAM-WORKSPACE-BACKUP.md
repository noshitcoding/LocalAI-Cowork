# Sicherung der Teamwerkzeug-Arbeitsbereiche

Der Compose- und Kubernetes-Lifecycle nimmt separat eingebundene Teamwerkzeug-
Arbeitsbereiche in das mit `age` verschlüsselte Gesamtbackup auf. Das zusätzliche
`team-workspaces.tar` enthält Dateien, leere Verzeichnisse, Dateimodi, SHA256 und
eine gebundene Eigentümerkennung. Die PostgreSQL-Sicherung, Schlüsselverwaltung,
Lauf-/Werkzeugquittungen und externe S3-Sicherung bleiben enthalten.

Ein konfigurierte Arbeitsbereich erhält `.cowork-workspace-owner.json`. Ein
Marker einer anderen Installation oder eines anderen Hostpfades wird abgewiesen.
Bereits konfigurierte Compose-Installationen registrieren den bestehenden,
ausdrücklich ausgewählten privaten UID1000-Ordner einmal mit dem bisherigen
`configure-team-tools`-Befehl, während ihre Dienste gestoppt sind.

Nach Stop der API-/Workerprozesse prüft der Lifecycle die tatsächliche lokale
Docker-Inventur. Noch lebende Gäste werden nur dann über ihre volle unveränderliche
Container-ID gestoppt, wenn ein unveränderter `ct-<nonce>.json`-Datensatz, Name,
Image, sämtliche gebundenen Labels und der genaue `/workspace`-Mount passen.
`State.Running=false`, `Pid=0`, kein Restart/Paused sind die Stopzeugen. Fremde,
unbestätigte oder neu auftauchende Writer blockieren die Sicherung. Container,
Credentialdateien und ursprüngliche Laufquittungen werden nicht entfernt oder
als erfolgreich bestätigt. Ein stehengebliebener Lauf benötigt weiterhin seine
ausdrückliche Recovery; die Sicherung wiederholt keine Werkzeuge.

Dateien mit Symlinks, Reparse Points, Hardlinks, speziellen Dateitypen, Traversal,
mehrdeutigen Namen oder wechselnder Identität werden abgewiesen. Grenzen sind
100000 Einträge, 2GiB pro Datei, 16GiB Inhalt und 32MiB Manifest. Ein Fehler lässt
die Dienste gestoppt; es wird keine ungeprüfte automatische Wiederaufnahme erzeugt.

Ein neues Compose-Restore verwendet standardmäßig den neuen privaten
`<deployment>/team-workspaces`-Ordner. `--team-tool-host-root` wählt stattdessen
einen neuen, noch nicht vorhandenen absoluten Ordner. Das ursprüngliche externe
Hostroot wird nicht übernommen. Beim ausdrücklichen Rollback bleiben neuere
Arbeitsdateien unter `.cowork-retained-before-restore-<UUID>` erhalten. Das
Hostroot und sein Eigentümermarker bleiben bestehen; es gibt keinen externen
Hostpfad-Purge. Alte Backups, die konfigurierte Teamtools haben, aber ihre
Arbeitsbereiche ausließen, werden als unvollständig abgewiesen.

Kubernetes verwendet eigene nodegebundene Wartungspods mit dem ausgewählten
Cowork-Image (Python und Docker-CLI), dem Data-PVC zum Lesen der Eigentümer-
datensätze und dem genauen Hostroot/Docker-Socket. Alle aktuell gelabelten und
zuvor gespeicherten Dockerworker-Nodes müssen mit derselben Node-UID erreichbar
und Ready sein. Jeder bestätigt seine eigenen Gäste; alle müssen denselben
Workspace-UUID-Marker sehen. Nur danach wird das gemeinsame Dateisystem gesichert.
Ein Ziel auf einem anderen Cluster braucht ein eigenes, zuvor bereitgestelltes
privates gemeinsames Hostroot im Installationsmanifest. Ein Hostroot einer
anderen aktiven Installation wird nicht adoptiert.

Neue Teamcontainerdatensätze (Format 2) binden die dauerhafte Operator-Host-UUID,
die positive Docker-Engine-ID und in Kubernetes Node-Name plus unveränderliche
Node-UID. Alle Bindungen werden zusätzlich als exakte Containerlabels gesetzt.
Die private append-only Inventur liegt unter `team-containers/hosts/`; sie bleibt
auch nach erfolgreichem Containercleanup erhalten. Der Backuphelper liest sie
vor der Nodeauswahl aus dem Data-PVC und berücksichtigt frühere Hosts unabhängig
von heutigen Schedulinglabels. Fehlende oder ersetzte Nodes/Engines und fremde
Hostdatensätze erlauben keine lokale Löschung oder eine Quieszenzbehauptung.
Inventardateien werden zuerst privat geschrieben und synchronisiert und dann
atomar ohne Ersetzen veröffentlicht (Linux `renameat2`, Windows `MoveFileExW`
ohne Replace). Ein nach einem Absturz verbliebener `publish-*.json`-Datensatz
wird erhalten und verweigert eine vollständige Inventur; er wird nicht als
bestätigte Hostbindung ausgegeben oder automatisch gelöscht.

Compose vergibt den Host-UUID beim Konfigurieren der Teamtools im eigenen
Operator-Deploymentmarker; ein neues Restore vergibt einen neuen UUID. Kubernetes
kopiert den lifecycle-eigenen ConfigMap `cowork-docker-hosts` als private reguläre
Datei in die Workerpods. Node-UIDs stammen aus der Kubernetes-API des ausdrücklich
gewählten Kontextes. Neu vom Operator gelabelte Nodes werden bei einer gestoppten
Lifecycle-Inventur mit neuen Host-UUIDs aufgenommen; bestehende Node-/Host-UUIDs
werden niemals ersetzt. Historische Nodes müssen weiterhin für die positive
Quieszenz erreichbar sein; eine verloren gegangene Engine wird nicht durch eine
neue Engine mit demselben Knotennamen adoptiert.
Die Aufnahme neuer Nodes geschieht beim Lifecycle-`stop` beziehungsweise vor
Backup/Update, nachdem alle Writerpods vollständig beendet wurden. Danach
starten neue Workerpods mit der aktualisierten privaten Bindungsdatei. Einfaches
Labeln eines Nodes während laufender Worker vergibt keine Hostausführungsrechte.
Eine automatisierte nachweisbare Stilllegung verlorener historischer Hosts
oder Migration ungebundener Altcontainer ist damit noch nicht implementiert.

Unbestätigte alte Format-1-Datensätze haben keine Hostbindung. Sie und ihre
Credentials bleiben erhalten, blockieren weitere Dateimutationen desselben
Auftrags und verhindern einen angeblich vollständigen neuen Workspacebackup.
Ein automatischer Hostwechsel oder das rückwirkende Erfinden einer Bindung ist
nicht vorgesehen. Solche Installationen benötigen eine ausdrücklich belegte
Recovery ihrer alten Datensätze. Backups kopieren private Inventar-/Laufdaten,
vergeben beim Restore aber keine Ausführungsrechte und starten keine alten Gäste.

Die sieben lokalen Negativ-/Roundtripfixtures und enge Kompilierung sind keine
physische Docker-, Kubernetes-, Failover- oder Produktionsabnahme.
