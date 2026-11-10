# Chrome und Firefox in V2

Der Browserbereich unter den Entwicklungswerkzeugen verwendet denselben
Sitzungsdienst wie das freigegebene KI-Werkzeug `browser`. Windows und Linux x64
unterstützen verwaltete Browser und Erweiterungen für installierte Browser.

## In der App

Unter **Werkzeuge einrichten** die private Laufzeit installieren und die
Werkzeugkonfiguration neu laden. Die Einrichtung lädt zusätzlich zu den bisherigen
Abhängigkeiten Chrome for Testing und Playwright-Firefox. Versionen und
SHA-256-Werte stehen in `tools/toolbox/browser-runtimes.lock.json`; die
Playwright-Version muss mit dem Lock übereinstimmen. Bestehende normale Browser
werden nicht verändert.

Im Browserbereich Chrome oder Firefox und **In der App** wählen, dann eine Sitzung
öffnen. **Übernehmen** aktiviert die manuelle Bedienung. Die Liveansicht unterstützt
Klicks, Tastatur, Scrollen, Tabs und verschiedene Ansichtsgrößen. **KI fortsetzen**
gibt die ausgewählte Sitzung an das Browserwerkzeug zurück. KI-Aktionen benötigen
weiterhin die konkrete bestehende Werkzeugfreigabe.

Profile liegen unter dem privaten Toolbox-Datenordner in
`browser-profiles/<project>/<engine>/profile`. Ein Betriebssystem-Dateilock verhindert
gleichzeitige Verwendung desselben Profils, auch durch einen zweiten Prozess.
Sitzungen schließen beendet den Browser, erhält aber Cookies und Anmeldungen.
**Projektprofil löschen** entfernt die gespeicherte Browsersitzung ausdrücklich.
Ein veränderter Browser-Binärstand verwendet ein bestehendes Profil nicht
unbemerkt weiter; für diesen Fall muss der ursprüngliche Stand wiederhergestellt
oder das Profil ausdrücklich gelöscht werden.

Die App zeigt Bilder des tatsächlichen Browserprozesses. Fremde Webseiten werden
nicht in den privilegierten Tauri-DOM eingebettet. Sichtbare Ansichten fordern
höchstens fünf Bilder pro Sekunde an; verdeckte Ansichten pausieren.

## Installierte Chrome- und Firefox-Profile

1. Aktuelle App beziehungsweise Geräteagent und `cowork-browser-host` bereitstellen.
2. Im lokalen App-Werkzeugsetup **Chrome- und Firefox-Erweiterungen → Einrichten /
   reparieren** wählen. Alternativ den Host mit `--register` starten.
3. Erweiterungen mit `npm run browser:build` bauen und mit
   `npm run browser:package` als reproduzierbare ZIPs paketieren.
4. Entwicklung: Chrome unter `chrome://extensions` mit Entwicklermodus und
   **Entpackte Erweiterung laden** auf `dist/browser-extensions/chrome` richten.
   Firefox unter `about:debugging#/runtime/this-firefox` als temporäres Add-on mit
   `dist/browser-extensions/firefox/manifest.json` laden.
5. Im Cowork-Browserbereich **Installierter Browser → Erweiterungen suchen** öffnen.
   Im Erweiterungs-Popup **Apps suchen**, die passende lokale Instanz und die Tabs
   auswählen und **Ausgewählte Tabs verbinden** auslösen.
6. In Cowork die entsprechende Verbindung wählen und eine Sitzung öffnen.

Der Browser fragt beim Verbinden nach Webseitenzugriff. Die Erweiterung erzwingt
zusätzlich eine Tabfreigabe: andere Tabs werden weder an Cowork gemeldet noch für
Aktionen verwendet. Private Fenster, interne Browserseiten und Datei-URLs sind
ausgeschlossen. Neu durch den Auftrag erzeugte HTTP(S)-Tabs gehören zur Freigabe.
Browserneustart oder **Zugriff beenden** entzieht die Tabfreigaben.

Die Erweiterung unterstützt Navigation, Verlauf, Tabs, Seiteninhalt, HTML-Formulare
und Scrollen. Screenshots erfordern einen aktiven freigegebenen Tab. Browser- oder
Websitefunktionen, die vertrauenswürdige native Eingaben verlangen, bleiben der
manuellen Bedienung im installierten Browser vorbehalten; die Erweiterung meldet
diesen Fall als nicht ausführbare beziehungsweise unklare Aktion.

## Verbindung und Geräteagent

`runtime.connectNative` startet den Rust-Host. Dieser erhält keine Daemon-,
Teamserver- oder Modellzugangsdaten. Er vermittelt ausschließlich das versionierte
Browserprotokoll über eine lokale, auf den Betriebssystembenutzer beschränkte
Named Pipe beziehungsweise einen Unix-Socket. Große Nachrichten werden begrenzt
und in geordnete Teilnachrichten zerlegt.

Der Host registriert eine eigene unveränderliche Kopie im lokalen Benutzerordner.
Dadurch bleiben temporäre AppImage-Mountpfade keine Laufzeitabhängigkeit.
Windows-Installation registriert und Deinstallation entfernt die Native-Messaging-
Registrierung für den aktuellen Benutzer. Linux richtet sie beim ersten echten
App-Start oder mit `--register` ein. Vor dem Entfernen eines Linux-Pakets oder
AppImages kann derselbe Benutzer `cowork-browser-host --unregister` ausführen;
auch im App-Setup gibt es **Registrierung entfernen**. Bei einem eigenen Chrome-
`--user-data-dir` muss die Native-Messaging-Konfiguration zusätzlich unter dessen
`NativeMessagingHosts` verfügbar sein. Der Befehl löscht keine
Browserprofile. `--diagnose` prüft die Konfiguration und die Portalvoraussetzungen.

Firefox aus Snap/Flatpak benötigt zusätzlich ein verfügbares WebExtensions-Portal
oder einen Native-Messaging-Proxy der Distribution. Die App verändert dafür keine
Browserpräferenzen oder Sandboxberechtigungen.

Ein gekoppelter Geräteagent startet den lokalen Browser-Hub, wenn die Browser-
Capability und native Benutzerrechte eingerichtet sind. Für Erweiterungen ohne
verwaltete Browser kann `--program browser_host=ABS/cowork-browser-host` registriert
werden. Das Browserwerkzeug bietet `connections`, um ausschließlich freigegebene
Tabs dieser lokalen Instanz aufzulisten. `engine`, `mode` und `connection_id`
erstellen eine Sitzung; die Projektkennung stammt aus dem authentifizierten
Auftrag, nicht aus Modellargumenten. Konkrete Aktionen verwenden anschließend
`session_id` und die bestehenden Serverfreigaben. Am Laufende werden die
Gerätesitzungen geschlossen; das Projektprofil bleibt erhalten.

## Protokoll und Wiederherstellung

Anfragekennung, Verbindungskennung, Sitzungs-Epoche und Dokumentstand werden
geprüft. Nach verlorenem Ergebnis wird keine Wirkung automatisch wiederholt.
Manuelle Eingaben behalten ausschließlich ihren Anfragefingerprint als Intent;
getippter Text wird dort nicht protokolliert. Ein identischer erneuter Request
liefert eine vorhandene Quittung oder meldet ein unklares Ergebnis. Passwort- und
versteckte Inputwerte sowie Skripte werden aus DOM-Beobachtungen entfernt.

Die Rust-Verträge erzeugen die TypeScript-Typen, Validatoren und OpenAPI-Routen.
Die bisherigen Chromium-Sitzungen und das `browser`-Werkzeug bleiben kompatibel.
`control` transportiert bei Bedarf einen typisierten `BrowserCommand` als JSON
im Feld `text`, einschließlich Tab-/Dokumentkennung und Koordinaten.

## Prüfung und Store-Auslieferung

- `npm test`, `npm run typecheck`, `npm run build`, `npm run contracts:check`
- `npm run browser:build` und `npm run browser:package`
- `cargo test -p cowork-toolbox browser_` und `cargo test -p cowork-browser-host`
- `tools/browser/smoke_managed.py`: echte Browser, lokale Testseiten, Profile,
  Eingaben, Livebilder, Entwicklerwerkzeuge und Cookie-Erhalt unter Windows/Linux.
- Der ignorierte Rust-Test `actual_browser_shares_manual_and_approved_actions_and_retains_profile`
  verbindet tatsächliche Browser mit dem vollständigen Toolbox-Freigabe-/Jobpfad.
  Er benötigt `COWORK_BROWSER_TEST_PYTHON`, `COWORK_BROWSER_TEST_EXECUTABLE` und
  `COWORK_BROWSER_TEST_ENGINE` und wird ausdrücklich mit `--ignored` gestartet.
- `smoke_extension_chrome.cjs` und `smoke_extension_firefox.py` laden die echten
  Erweiterungspakete in frische Testprofile. Webseiten- und Browseraktionen sind
  real; die native Gegenstelle ist eine deterministische Fixture. Rust-Tests
  prüfen Transport, Grenzen, Freigaben und Verbindungsabbrüche getrennt.
- `smoke_native_chain.py --peer <browser_bridge_acceptance> --host
  <cowork-browser-host>` prüft beide Erweiterungen über den tatsächlichen Rust-Host,
  OS-Transport und Toolbox-Freigabe-/Jobpfad. Das Beispiel entsteht mit
  `cargo build -p cowork-toolbox --example browser_bridge_acceptance`. Der Test
  benötigt Selenium, die privaten Testlaufzeiten und frische Browserprofile;
  zufällige temporäre Native-Host-Registrierungen werden anschließend entfernt.
  `COWORK_BROWSER_TEST_MODEL_ACTIONS` kann einen tatsächlich lokal erzeugten
  Modellplan an alle Laufzeittests übergeben.

Diese Prüfungen sind keine Abnahme eines externen Modellanbieters, einer
Store-Veröffentlichung oder aller Snap-/Flatpak-Distributionen.

Für Chrome Web Store und AMO siehe `extensions/browser/STORE-SUBMISSION.md`.

Der tatsächlich geprüfte Stand einschließlich lokaler Qwen-Aufgabe und genauer
Abnahmegrenzen steht in [Browserprüfung vom 18.10.2026](BROWSER-VERIFICATION-2026-10-18.md).
