# Aktuelle Installationslieferung: 2.0.0-alpha.6

Der Stand vom 14. Oktober 2026 enthält die vervollständigten V2-Quellen bis
Schema75, den eigenständigen Geräteagenten und die aktuelle Weboberfläche.
Der schnelle Lieferlauf baut native Windows-/Linux-Dateien und Pakete; er
führt auf Wunsch keine umfangreichen Tests oder Installation aus.
Die Dateien bleiben unsigniert. Ein öffentlicher Upload und eine produktive
Laufzeitabnahme sind nicht Bestandteil dieser Lieferung. Alte Belege unten
behalten ausschließlich ihren damaligen Quellen- und Binärumfang.

## Installation auf dem eigenen Rechner und Server

Das aktuelle Gesamtarchiv enthält Windows-NSIS unter `installers/windows`,
Linux-DEB und AppImage unter `installers/linux`, das Webfrontend unter `web`
und die Geräteagenten samt Einrichtung unter `device-agent`.
`DELIVERY-RECEIPT.json`, `SOURCE-PINS.json`, `SHA256SUMS` und die SBOMs binden
die tatsächlich gelieferten Dateien. Das zusätzliche `SOURCES.zip` enthält
die vollständigen Produktquellen für einen eigenen Build.

Für Windows den enthaltenen Installer starten. Unter Linux das DEB installieren
oder die AppImage-Datei ausführbar machen und starten. Die lokale App kann
zunächst ohne externen Server verwendet werden. Eine eigene Serveradresse wird
später in der Verbindungsverwaltung eingetragen; lokale Ordnerfreigaben und
Zugangsdaten werden dabei nicht automatisch auf den Server übertragen.

Für den V2-Server liegt `server/localai-cowork-server-linux-x64.tar.gz` bei.
Die enthaltene Serveranleitung erklärt die native Installation. Für einen
Compose-Aufbau die Quellen aus `SOURCES.zip` entpacken und
[deploy/v2/README.md](../deploy/v2/README.md) einschließlich Initialisierung,
TLS, PostgreSQL, optionalem S3 und Backup/Restore verwenden. Ein neuer Server
benötigt eigene private Schlüssel und Zugangsdaten. Teams erhalten jeweils
ihren eigenen LiteLLM-Zugang; interne Token- oder Geldbudgets sind nicht nötig.

Der getrennte Enterprise-Server wird zusätzlich als
`open-cowork-enterprise-source.zip` geliefert. Er enthält seinen eigenen
Docker-Buildkontext, das tatsächlich gebaute Webfrontend, Installationsanleitung,
Quellenpins, Abhängigkeitsinventare und Prüfsummen. Im entpackten Paket zuerst
`INSTALL.txt` und `source/deploy/enterprise/README.md` lesen. Die Images werden
auf dem eigenen Server gebaut und mit den dort eingerichteten Schlüsseln,
DNS/TLS-, PostgreSQL/S3- und Ressourcenangaben bereitgestellt. Dieses getrennte
Paket ist der Enterprise-Pfad; der V2-Daemon allein ersetzt dessen Management,
VM-/Kubernetes-Controller und Instanzverwaltung nicht.

Die Geräteagenten mit dem jeweils nativen Programm und dem enthaltenen privaten
Profilkonfigurator einrichten. Unter Linux nach dem Entpacken dem enthaltenen
Agenten mit `chmod +x device-agent/Linux/cowork-device-agent-v2` das Ausführungsrecht
geben. Modell-/LiteLLM-Zugang, erlaubte Programme, Office/MCP und Ordnerfreigaben
werden am jeweiligen Gerät ausdrücklich gewählt.
Anleitung: [DEVICE-AGENT-V2.md](DEVICE-AGENT-V2.md).
Externe Programme und VM-/Container-Laufzeiten müssen auf dem Zielsystem
vorhanden sein. Der aktuelle Funktionsumfang und konkrete Betriebsgrenzen stehen
in FEATURE-MATRIX.md (development artifact omitted from public sources).

## Historische Lieferung vom 10. Oktober 2026


Dieser Stand bereitet `2.0.0-alpha.1` als unsignierte persönliche Preview vor.
Ein externer Server wird vom Benutzer später selbst eingerichtet. Es wurden
keine öffentlichen Releases angelegt und keine Anwendungen im normalen
Benutzerprofil installiert.

## Aktuelle Nachweise

Der Hybrid-Corebeleg (private development evidence; omitted from public sources)
bindet 1905 aktuelle Quellen und 326 Belegdateien. Die früheren vollständigen
Workspace-Läufe mit 565 Windows-/548 Ubuntu-Tests bleiben historisch. Genau
zwei Coredateien wurden danach korrigiert; 1903 Eingaben sind bytegleich.
Aktuell bestehen 50 optimierte Windows- und 18 gezielte Linux-Tests sowie
zwölf native Daemon-Prozesssuiten je Plattform. Die Prozesssuiten enthalten
je 81 ausdrücklich erwartete Fixture-Hardstops, die als solche bezeichnet
bleiben. Alle eigenen Prozesse und Providerthreads sind positiv beendet.

Der Windows-Paketbeleg (private development evidence; omitted from public sources)
bindet den aktuellen NSIS-Installer, Sidecar und 1929 Eingaben. Acht native
Desktoptests, Format und Clippy sowie Paketierung bestehen. Die exakt
entpackte Payload startet im eigenen privaten Windows-WebView.
Der native Verbindungswechsel (private development evidence; omitted from public sources)
prüft lokalen Arbeitsplatz → eigenen kompatiblen Testserver → lokalen
Arbeitsplatz. Die Serververbindung erhält keine lokalen Sicherheits- oder
Ordnerrechte. Die native Modell-Demo (private development evidence; omitted from public sources)
legt Projekt, Auftrag und Profil an und zeigt eine echte gespeicherte
`qwen3:4b`-Antwort `Bereit`. Der endgültige Lauf meldet 1349 Eingabe- und
194 Ausgabetoken ohne Werkzeugaktionen. Ein früherer privater Decoderfehler
bleibt als Fehlbeleg erhalten; insgesamt fanden zwei private Modellaufrufe
statt. Preis- oder Tokenizerparität wird daraus nicht abgeleitet.

Das Windows-Desktopfenster endet regulär. Sein eigener Daemon bleibt danach
erreichbar; zur Testbereinigung wurde genau dieser Daemon ausdrücklich
beendet. Das ist kein geordneter Daemonstop durch das Desktopfenster und keine
Prüfung der Registrierung durch den NSIS-Installationsdialog.

Der aktuelle Browser-Demobeleg (private development evidence; omitted from public sources)
bindet 1980 Eingaben, 202 Frontendtests und neun echte Browser-Prüfgruppen mit
optimiertem Daemon. Er prüft zusätzlich exakte Geldbeträge, unbekannte Kosten,
fehlende Konfiguration, sichtbar veraltete Werte und Neustart.

Das Serverarchiv (private development evidence; omitted from public sources)
enthält den aktuellen Linux-Release-Daemon und das aktuelle Webfrontend.
Eine private Installation, zwei normale Worker-Lifetimes und 14 HTTP-/Auth-/
Origin-/Webprüfungen bestehen. Alle 16 regulären Archivdateien und 15
Prüfsummeneinträge wurden unabhängig erneut geprüft. Beide Daemons endeten
geordnet; ihre privaten Testdaten und Schlüsseldateien wurden entfernt.

Die Linux-Payloadprüfung (private development evidence; omitted from public sources)
bindet die tatsächlichen Debian-/AppImage-Pakete an 1929 aktuelle Eingaben.
Das Debian-Paket enthält zusätzlich zu GTK/WebKit tatsächlich
`libc6 (>= 2.39)`. Für das AppImage wurden Marker und RPATH-Transformation
mit dem tatsächlich eingebetteten Patchelf vollständig nachgespielt;
die gesamten resultierenden Desktop- und Sidecardateien sind bytegleich.
Damit werden keine beliebigen Unterschiede akzeptiert. Die aktuelle
ELF-Mindestversion ist glibc 2.39 für den Desktop und 2.38 für den Sidecar.
Das finale Linux-Siegel (private development evidence; omitted from public sources)
bindet außerdem zwei sichtbar gerenderte native Fensterläufe. Das tatsächliche
AppImage startet mit `--appimage-extract-and-run`; beim Debian-Paket startet
die exakt entpackte Desktop-Payload. Beide Fenster schließen über das normale
Fensterprotokoll mit Exitcode 0. Der jeweilige eigene Daemon bleibt erreichbar
und endet danach auf sein gezieltes SIGINT. Alle eigenen Prozess-Handles sind
positiv abgeschlossen. Dies belegt keine Systeminstallation des Debian-Pakets.

Bei einer Serververbindung sind einige Projekt-, Lauf- und Dateidetailtexte
noch lokal formuliert. Die Ausführung und Speicherung erfolgen auf dem im
Verbindungsdialog ausgewählten Server. Diese verbleibende Anpassung der
Detailanzeigen ist in der Demoanleitung ausdrücklich aufgeführt.

## Bereinigung und verbleibender Auftrag

Der eigene Windows-Targetcache ist nach Freigabe seiner Besitzer mit Cargo
bereinigt. Quellen, unabhängige Paketbytes und unveränderliche Belege sind
erhalten. Der Linux-Cargocache (private development evidence; omitted from public sources)
wurde danach ebenfalls gezielt mit Cargo bereinigt: 18.607 Builddateien und
19,7 GiB entfernt. Alle 1929 Quellen, Paketbytes und Replaybelege sind vor
und nach der Bereinigung identisch. Zusammen wurden über 40 GB eigener
Builddateien entfernt. Auch die zwei eigenen Browser-Datenordner wurden nach Versiegelung
entfernt; ursprüngliche Seeds und normale Benutzerprofile blieben erhalten.

Eine zusätzliche begrenzte read-only Inventarisierung weist installierte
V1-Programme `Local AI Cowork 0.3.12` und `Open Cowork 0.1.8` nach. Sie liest
nur Programmidentität/Version/SHA und Installationsmetadaten. Kein Programm
wurde gestartet, keine Datenbank geöffnet und keine Registry geändert.
Source-/Build-/Datenparität und der vollständige M0-Nachweis bleiben offen.

Der vollständige M0–M10-/Serverauftrag ist weiterhin offen. Insbesondere
werden Live-Migration, Enterprise/V1-Brücke, Offline-/Gerätesynchronisierung,
vollständige Tool-/Sandboxqualifikation, Last-/Barrierefreiheitsabnahme und
Signatur-/Updatekette durch diese Preview nicht abgeschlossen.
[Installation und Demo](DEMO-AND-INSTALLATION.md), [Release-Text](RELEASE-NOTES-2.0.0-alpha.1.md)
und [verbindlicher Restumfang](COMPLETION-WORKPLAN.md) begleiten die Lieferung.
