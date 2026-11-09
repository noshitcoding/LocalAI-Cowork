# Gespeicherte Desktopupdates und Wiederherstellung

Der native Updateweg lädt und prüft die signierten HTTPS-Paketbytes, bevor er
den passenden lokalen Daemon zum geordneten Stop auffordert. Renderer können
keine Zieladresse, Credentials, PID oder Dateipfade für diese Aktion vorgeben.

Neue Vorgänge verwenden unveränderliche Belege im Format 3 unter dem privaten
V2-Datenordner `update-receipts`. Jeder Beleg bindet Update-ID, Paket-SHA256,
Versionen sowie die ursprüngliche Daemoninstanz. Phasen binden zusätzlich den
SHA256 der exakten ursprünglichen Belegbytes. Eine atomare Veröffentlichung
ersetzt keinen vorhandenen Beleg. Ein Betriebssystem-Dateilock serialisiert
Updateänderungen und lokale Daemonstarts auch zwischen mehreren Appprozessen.

Vor dem Stop startet die App einen eigenen Beobachter derselben Programmdatei.
Windows überträgt ausschließlich ein synchronisierbares Handle des zuvor
geprüften Daemons; Linux überträgt dessen PIDfd nur im eigenen Kindprozess.
Der Beobachter verwendet keine erneute PID-Suche. Erst das positive native
Exitereignis erzeugt `daemon-stopped`. Er kann diesen Beleg auch nach GUI-Absturz
oder Beobachtungszeitüberschreitung schreiben. Die App wartet vor Installation
zusätzlich auf das tatsächliche Ende ihres eigenen Beobachters.

Beim Appstart und vor jedem lokalen Daemonstart werden die gespeicherten
Vorgänge neu geprüft. Unbestätigte oder beschädigte Vorgänge bleiben gesperrt.
Die Updateansicht ist auch ohne Daemonverbindung am Verbindungsbildschirm
erreichbar. „Updatevorgänge prüfen“ liest den aktuellen Stand. Nach bestätigtem
Daemonende und vor Beginn eines Installers kann „Belegten Vorgang abschließen“
den ursprünglichen Vorgang ausdrücklich schließen. Das startet keine
Installation und wiederholt keinen Modell-, Werkzeug- oder Updateaufruf.

Ein belegter Installationsfehler kann auf demselben Weg geschlossen werden.
Linux zeichnet eine erfolgreich zurückgekehrte atomare Paketinstallation auf;
der Neustart akzeptiert diesen Abschluss nur für die tatsächlich laufende
Zielversion.

Windows verwendet einen unabhängigen Installerbeobachter aus einer privaten,
hashgebundenen Kopie der ursprünglichen App. Die Kopie hält die zu ersetzende
installierte EXE nicht offen. Signaturgeprüfte NSIS-Bytes werden vor dem Daemonstop
atomar und ohne Überschreiben abgelegt. Ihr signiertes PE-Versionsresource muss
den Vertrag `nsis.currentUser.originalProcess.v1` und die genaue Zielversion
enthalten. `windows/update-hooks.nsh` bindet diesen Vertrag beim Paketbau;
der ausgelieferte Installationsmodus ist ausdrücklich `currentUser`.

Der Beobachter erhält ausschließlich ein synchronisierbares Handle der
ursprünglichen GUI. Er prüft Paket-/Beobachterhashes und das unveränderliche
Installationsziel, bestätigt seine Bereitschaft und wartet anschließend auf
das positive GUI-Exitereignis. Erst der bereits gespeicherte ausdrückliche
`install-requested`-Beleg und der ursprüngliche Daemonstop erlauben einen
einzigen Start. Ein vorzeitig beendeter Appvorgang ohne diese Freigabe startet
keinen Installer. Die GUI beendet sich geordnet nach der Übergabe.

`ShellExecuteExW` liefert das eigene Installer-Prozesshandle. Seine tatsächliche
Programmdatei wird mit dem festgehaltenen Paketpfad verglichen. Der Beobachter
wartet ohne Timeout-als-Erfolg auf das native Prozessende und speichert den
Exitcode beziehungsweise einen belegten Startfehler in `installer-result.json`.
Das Ergebnis bindet ursprünglichen Vorgang, Installationsplan und Paket-SHA256.
Es bleibt auch dann auswertbar, wenn die folgende Komfortphase nicht mehr
geschrieben werden konnte. Ein erfolgreicher Abschluss öffnet den Daemonstart
erst in der tatsächlich laufenden Zielversion. Ein belegter Fehler lässt sich
ausdrücklich ohne erneute Installation schließen.

Der NSIS-Aufruf verwendet `/P /UPDATE /D=<Originalziel>`; `/D` steht unquotiert
am Ende. Es gibt keinen automatischen Installer-/Appstart beim Wiederöffnen.
Die mitgelieferten Hooks ersetzen Tauri-Prozesskills nach Executablename durch
die Verfügbarkeitsprüfung der exakten installierten Programmdatei. Ein noch
verwendetes oder gesperrtes Programm führt zum Installerabbruch.

Ein fehlendes natives Handle, beschädigte Belege oder ein Verlust des
Beobachters nach Dispatch bleiben ungeklärt. Versionsnummer oder Portabwesenheit
ersetzen diesen Beleg nicht. MSI und abweichende NSIS-Ausführungsmodelle werden
nicht als der unterstützte NSIS-Vertrag ausgegeben. Ältere Installer ohne
Vertragsresource sind davon nicht nachträglich qualifiziert.

Historische Format-2-Vorgänge ohne übertragene Prozessautorität werden nicht
automatisch als gestoppt umgedeutet. Die gespeicherten Belege bleiben erhalten.
Signierkonfiguration, neu gebaute Gesamtpakete, tatsächliche Installation und
die vollständige physische Windows-/Linux-Updateabnahme sind eigene Liefer-
und Qualifizierungsschritte. Die kleinen Journal-/Native-Handleprüfungen
ersetzen diese Plattformabnahme nicht.

Die verwendeten API-Verträge sind die [Microsoft-Shellprozesshandles](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow),
die [NSIS-Zielparameter](https://nsis.sourceforge.io/Docs/Chapter3.html) und die
[gepinnten Tauri-2.11.4-Installerhooks](https://raw.githubusercontent.com/tauri-apps/tauri/tauri-cli-v2.11.4/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi).
