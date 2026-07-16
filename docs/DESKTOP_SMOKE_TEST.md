# Desktop Smoke Test

## Ziel

Diese Checkliste validiert den lokalen Windows-Desktop-Pfad fuer Open_Cowork reproduzierbar nach UI-, Engine- oder Tauri-Aenderungen.

## Voraussetzungen

- Windows mit WebView2 Runtime
- Node.js, npm und Rust Toolchain installiert
- Ollama erreichbar unter dem konfigurierten Endpoint
- Workspace sauber installierbar (`npm install` in `app`)

## Vorbereitung

```powershell
cd app
npm install
npm run test:ci
npm run build
```

Erwartung:
- Frontend-Tests grün
- Produktions-Build erfolgreich

## Smoke-Start

```powershell
cd app
npm install
npm run test:ci
npm run build
```

Erwartung:
- Frontend-Tests grün
- Produktions-Build erfolgreich

## Smoke-Start

```powershell
cd app
npm run smoke:desktop
```

Erwartung:
- Rust kompiliert ohne Fehler
- das Release-Binary wird unter `src-tauri/target/release/app.exe` erzeugt
- das Binary bleibt beim Start stabil und stellt innerhalb von 20 Sekunden ein Fenster mit dem Titel `Open Cowork` bereit
- der automatisierte Smoke-Prozess wird nach erfolgreicher Pruefung wieder beendet

## Durchklickrunde

1. Startansicht pruefen
Erwartung:
- WelcomeScreen oder aktiver Cowork-Thread wird angezeigt
- Top-Navigation fuer Cowork und Settings ist sichtbar

2. Settings oeffnen
Erwartung:
- die 9 Settings-Kategorien sind erreichbar
- in KI & Modell sind Ollama-Endpoint und Modell sichtbar

3. Health-Check ausfuehren
Erwartung:
- Ollama-Status wird als verbunden oder mit konkreter Fehlermeldung angezeigt
- Modelle koennen geladen werden

4. Neuen Chat anlegen
Erwartung:
- Sidebar legt einen neuen Thread an
- Cowork-Ansicht wird aktiv

4b. Sidebar-Status klarstellen
Erwartung:
- Projektlisten zeigen klarer an, wenn ein Bereich leer ist
- Leere Bereichsbeschreibungen sind für Chat- und Session-Leerzustände verständlich

5. Prompt senden
Beispiel:
- "Pruefe den aktuellen Build-Status des Projekts"
Erwartung:
- User-Nachricht erscheint im Thread
- Engine-Antwort streamt oder liefert eine sichtbare Fehlermeldung

6. Slash-Kommandos fuer lokalen Betrieb pruefen
Kommandos:
- `/ollama`
- `/local-model`
- `/local-runtime`
Erwartung:
- `/ollama` wechselt in die lokalen Ollama-Einstellungen
- `/local-model` bestaetigt aktuelles Modell und verweist auf lokale Modellwahl
- `/local-runtime` bestaetigt lokalen Desktop-/Ollama-Betrieb ohne Cloud-Backend

7. Persistierte Session aus der Sidebar laden
Erwartung:
- Bereich "Persistierte Sessions" ist sichtbar, sobald Sessions existieren
- Klick auf eine Session hydriert den Thread in die Cowork-Ansicht
- die geladene Session wird aktiver Thread

8. App schliessen und neu starten
Erwartung:
- bestehende Threads sind weiter verfuegbar
- zuletzt genutzte Sessions koennen erneut geladen werden

## Desktop-Steuerung validieren

Diese Runde prueft die lokalen Desktop-Tools fuer Modelle wie Gemma/Ollama. Sie ist getrennt von OpenAI Computer Use.

1. Screenshot aufnehmen
Beispielprompt:
- "Nimm einen DesktopScreenshot auf und beschreibe die sichtbaren Koordinatenhilfen."

Erwartung:
- Ein Bild-Attachment wird angehaengt.
- Das Bild enthaelt ein Koordinatenraster.
- Die Antwort nennt lokale Display-Koordinaten mit Ursprung links oben.

2. Fenster fokussieren
Beispielprompt:
- "Liste sichtbare Fenster und fokussiere das Open_Cowork-Fenster."

Erwartung:
- `DesktopListWindows` liefert Fenstertitel, Prozess und Bounds.
- `DesktopFocusWindow` fuehrt eine Fokus-Anfrage aus.
- Danach wird ein Verifikations-Screenshot angehaengt.

3. Unkritischen Klick testen
Beispielprompt:
- "Nutze den Screenshot, waehle einen sicheren leeren Bereich in Open_Cowork und bewege die Maus dorthin, ohne zu klicken."

Erwartung:
- Das Modell nutzt `DesktopMoveMouse`.
- Die Koordinaten werden als Display-Koordinaten interpretiert.
- Es wird kein destruktiver UI-Schritt ausgefuehrt.

## OpenAI Computer Use Debugging

`ComputerUseAppTest` ist die OpenAI-basierte Debugging-Schnittstelle fuer Vibecoding. Sie ist kein MCP-Server und nicht der lokale Gemma/Ollama-Pfad.

Voraussetzungen:
- OpenAI API-Key in den Einstellungen hinterlegt
- Modell `computer-use-preview` oder kompatibel konfiguriert
- Ziel-App oder Ziel-Fenster ist lokal sichtbar

Beispielprompt:
- "Fuehre mit ComputerUseAppTest einen kurzen UI-Test aus: fokussiere Open_Cowork und pruefe, ob die Settings erreichbar sind. Maximal 3 Schritte."

Erwartung:
- Open_Cowork sendet Screenshots an die OpenAI Responses API.
- OpenAI liefert Computer-Aktionen zurueck.
- Open_Cowork fuehrt die Aktionen lokal aus.
- Das Ergebnis enthaelt Status, ausgefuehrte Schritte und einen kurzen Testbericht.

Details:
- [DESKTOP_CONTROL_AND_COMPUTER_USE.md](DESKTOP_CONTROL_AND_COMPUTER_USE.md)

## Fehlerprotokoll

Bei einem Fehler festhalten:

- Commit oder Arbeitsstand
- Schritt aus dieser Checkliste
- sichtbares Verhalten
- Build-Ausgabe aus `npm run tauri build`
- falls relevant: Ollama-Endpoint und Modell

## Minimalfreigabe

Der Smoke-Test ist bestanden, wenn alle folgenden Punkte zutreffen:

- `npm run test:ci` erfolgreich
- `npm run build` erfolgreich
- `npm run smoke:desktop` baut und startet die native Desktop-App erfolgreich
- Settings und Cowork sind erreichbar
- ein Prompt kann gesendet werden
- persistierte Sessions lassen sich direkt aus der Sidebar laden
