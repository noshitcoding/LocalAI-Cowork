# LocalAI Cowork V2 — 2.0.0-alpha.1

Diese Alpha stellt den persönlichen V2-Arbeitsplatz als native Desktop-App
und als selbst installierbares Linux-Serverpaket bereit.

## In dieser Preview

- Windows-x64-Installer sowie Linux-x64-Pakete als Debian und AppImage.
- Gebündelter lokaler Daemon für den Desktop; ein externer Server ist optional.
- Eigenes Serverpaket mit Webfrontend, Installationshelfer, Prüfsummen und
  Vorlagen für systemd und nginx.
- Dauerhafte Projekte, Aufträge und Modellläufe mit ausdrücklich auswählbaren
  Modellanbietern. Bestehende Ollama-Modelle können verwendet werden.
- Persönlicher Geldstatus mit exakten Dezimalbeträgen, getrennten bestätigten,
  ausstehenden und unbekannten Kosten sowie sichtbarem Aktualisierungsstatus.
- Ausdrücklicher Wechsel zwischen lokalem Arbeitsplatz und kompatiblem
  V2-Server. Eine manuelle Serververbindung erteilt keine lokale Ordner- oder
  Schlüsselautorität.
- Korrigierter Windows-Release-Start und zusätzliche Prüfungen für vollständige
  Autoritätsdateinamen sowie Unicode-Dateipublikation.

Anleitung: [Installation und Demo](DEMO-AND-INSTALLATION.md).
Geldkonfiguration: [Private persönliche Geldbudgets](PERSONAL-MONEY-CONFIGURATION.md).

## Voraussetzungen

Windows benötigt x64 und WebView2. Der Linux-Desktop benötigt x64 und glibc
mindestens 2.39 mit GTK3/WebKitGTK 4.1; geprüft wurde Ubuntu 24.04. Ein AppImage
senkt die glibc-Mindestversion nicht. Der separate Server benötigt x64, glibc
mindestens 2.38 und `libgcc_s`; die tatsächlichen Serverstarts wurden unter
Ubuntu 24.04 in WSL2 geprüft. Modelle und Providerzugänge werden selbst eingerichtet.

## Status

Die Pakete sind unsigniert. Quellen, Projektlizenz, verfügbare Lizenzdateien
gelockter Abhängigkeiten und SHA256-Prüfsummen werden mitgeliefert. Ein Upload,
eine öffentliche Veröffentlichung oder eine Installation im normalen
Benutzerprofil wurden nicht vorgenommen.

Einige Projekt-, Lauf- und Dateidetailtexte sind bei Serververbindungen noch
lokal formuliert. Speicherung und Ausführung erfolgen auf dem ausgewählten
Server; diese Detailanzeigen bleiben als bekannte UI-Anpassung offen.

Der vollständige M0–M10-/Serverumfang ist weiterhin offen. Insbesondere sind
Enterprise/V1-Brücke, produktive Live-Migration, Gerätesynchronisierung,
vollständige Tool-/Sandboxqualifikation, Last- und Barrierefreiheitsabnahme sowie
Signatur-/Updatekette keine abgeschlossenen Funktionen dieser Alpha.
Den verbindlichen Restumfang beschreibt [der vollständige Arbeitsplan](COMPLETION-WORKPLAN.md).

Die Lieferung ist für eine Veröffentlichung als **Alpha/Preview** vorbereitet.
Die Paket- und Testquittungen dokumentieren den jeweils tatsächlich geprüften
Umfang; sie ersetzen keine vollständige Produktionsabnahme.
