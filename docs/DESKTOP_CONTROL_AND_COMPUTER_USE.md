# Desktop-Steuerung und Computer Use

Diese Seite beschreibt die drei unterschiedlichen Desktop-Steuerungswege in Open_Cowork. Die Trennung ist wichtig, weil die Namen aehnlich klingen, aber unterschiedliche Modelle und Schnittstellen verwenden.

## Uebersicht

| Pfad | Zweck | Modell | Kann sehen? | Kann klicken/tippen? |
|---|---|---|---|---|
| Lokale Desktop-Tools | Standardweg fuer Gemma/Ollama und lokale Agenten | Ollama/lokale Modelle | Ja, ueber `DesktopScreenshot` | Ja, ueber `DesktopClick`, `DesktopKeypress`, `DesktopTypeText`, `DesktopScroll` |
| Screenshot-MCP | MCP-kompatibler Screenshot-Zugriff | jedes Modell mit MCP-Toolzugriff | Ja | Nein |
| `ComputerUseAppTest` | Debugging-Schnittstelle fuer Vibecoding mit OpenAI Computer Use | OpenAI `computer-use-preview` | Ja | Ja, aber Aktionen kommen von OpenAI und werden lokal ausgefuehrt |

## 1. Lokale Desktop-Tools fuer Gemma/Ollama

Das ist der normale lokale Computer-Control-Pfad.

Der Agent nutzt einen Loop:

1. `DesktopScreenshot` erstellt einen Screenshot des Primaerdisplays.
2. Das Bild wird als Attachment an das Modell gegeben.
3. Das Bild enthaelt ein Koordinatenraster.
4. Das Modell waehlt lokale Display-Koordinaten aus dem Bild.
5. Der Agent fuehrt eine Aktion aus, z. B. `DesktopClick`.
6. Nach der Aktion wird automatisch ein Verifikations-Screenshot angehaengt.

Wichtige Tools:

- `DesktopScreenshot`: Screenshot mit Koordinatenraster.
- `DesktopPrimaryDisplay`: Display-Geometrie und Ursprung.
- `DesktopListWindows`: sichtbare Fenster mit Bounds.
- `DesktopFocusWindow`: Ziel-Fenster fokussieren.
- `DesktopLaunchApp`: Windows-App starten.
- `DesktopMoveMouse`: Maus bewegen.
- `DesktopClick`: Maus klicken.
- `DesktopTypeText`: Text in das fokussierte Fenster eingeben.
- `DesktopKeypress`: Tastenkombinationen senden.
- `DesktopScroll`: scrollen.

Koordinatenregel:

- Standard ist `coordinate_space="display"`.
- `x` und `y` sind dann Pixelkoordinaten im zuletzt gesehenen Screenshot.
- Ursprung ist links oben im Screenshot: `(0, 0)`.
- Bei Mehrmonitor-Setups rechnet Open_Cowork den Display-Ursprung automatisch in absolute virtuelle Bildschirmkoordinaten um.
- `coordinate_space="screen"` ist nur fuer absolute Windows-Bildschirmkoordinaten gedacht.

## 2. Screenshot-MCP

Der lokale MCP-Server heisst:

```text
open-cowork-screenshot-mcp
```

Dieser Server ist absichtlich nur fuer Screenshots gedacht. Er ist kein vollstaendiger Computer-Use-Server.

Bereitgestellte Tools:

- `list_screens`: angeschlossene Screens mit Index, Position, Groesse und Primary-Status.
- `capture_screenshot`: Screenshots aller Monitore als PNG-Dateien speichern.
- `screenshot_for_display`: Screenshot als Bilddaten plus Metadaten zurueckgeben.

`screenshot_for_display` gibt zusaetzlich zur Bilddatei Metadaten zurueck:

- `displayInfo.width` / `displayInfo.height`
- `displayInfo.x` / `displayInfo.y`
- `displayInfo.deviceName`
- `displayInfo.scaleFactor`
- `coordinateOverlay`
- `coordinateGrid`

Das Screenshot-Bild enthaelt ein Raster:

- feine Linien alle 50 px
- beschriftete Hauptlinien alle 100 px
- Ursprung links oben
- Koordinatenraum: lokale Display-Koordinaten

Wichtig: Der Screenshot-MCP kann nicht klicken, tippen oder scrollen. Fuer Aktionen muessen die lokalen Desktop-Tools verwendet werden.

## 3. `ComputerUseAppTest`

`ComputerUseAppTest` ist keine MCP-Schnittstelle. Es ist eine Debugging- und Test-Schnittstelle fuer Vibecoding mit OpenAI Computer Use.

Der Ablauf:

1. Open_Cowork nimmt lokal einen rohen Desktop-Screenshot auf.
2. Open_Cowork sendet Screenshot und Zielbeschreibung an die OpenAI Responses API.
3. Die Anfrage aktiviert das OpenAI-Tool `computer_use_preview`.
4. OpenAI antwortet mit einer Computer-Aktion, z. B. Klick, Scroll, Tastendruck oder Texteingabe.
5. Open_Cowork fuehrt diese Aktion lokal ueber die Windows/Tauri-Desktop-Kommandos aus.
6. Danach nimmt Open_Cowork einen neuen Screenshot auf.
7. Der Loop laeuft bis `completed`, `blocked` oder `max_steps`.

Konfiguration:

- API-Key: OpenAI API-Key in den Einstellungen.
- Modell: normalerweise `computer-use-preview`.
- Base URL: OpenAI Responses API kompatibler Endpoint.
- `max_steps`: maximale Anzahl Computer-Use-Aktionen.
- `action_delay_ms`: Wartezeit nach jeder Aktion vor dem naechsten Screenshot.
- `launch_delay_ms`: Wartezeit nach App-Start.
- `auto_acknowledge_safety_checks`: nur in kontrollierten Testumgebungen aktivieren.

Warum der Name historisch ist:

- `ComputerUseAppTest` wurde als Debugging-Schnittstelle fuer App-/UI-Tests gebaut.
- Das Tool nutzt OpenAI Computer Use, nicht den lokalen Screenshot-MCP.
- Fuer lokale Modelle ist der Name daher leicht missverstaendlich.

Empfohlene gedankliche Trennung:

- `ComputerUseAppTest` = OpenAI-basierter Debug-/Vibecoding-Testloop.
- `DesktopScreenshot` plus `DesktopClick` usw. = lokaler Gemma/Ollama Desktop-Control-Loop.
- `open-cowork-screenshot-mcp` = MCP-Screenshot-Provider ohne Aktionssteuerung.

## Wann welchen Pfad nutzen?

Nutze lokale Desktop-Tools, wenn:

- Gemma/Ollama den Desktop steuern soll.
- du volle lokale Kontrolle ohne OpenAI API brauchst.
- du Screenshot-Raster und explizite Koordinaten verwenden willst.

Nutze Screenshot-MCP, wenn:

- ein MCP-faehiger Agent Screenshots als Tool abrufen soll.
- du Bilddaten und Display-Metadaten ueber MCP bereitstellen willst.
- keine direkte Maus-/Tastatursteuerung ueber MCP erforderlich ist.

Nutze `ComputerUseAppTest`, wenn:

- du OpenAI Computer Use fuer UI-Debugging verwenden willst.
- du schnell einen App-Workflow explorativ testen willst.
- du eine Vibecoding-Debugging-Schnittstelle brauchst, die Screenshots und Aktionen automatisch loopt.

## Bekannte Namensfalle

Der Begriff "Computer Use" kann zwei Dinge meinen:

1. Allgemein: ein Modell sieht den Bildschirm und steuert Maus/Tastatur.
2. Spezifisch: OpenAI `computer_use_preview`.

In Open_Cowork meint `ComputerUseAppTest` die zweite Bedeutung. Fuer allgemeine lokale Desktop-Steuerung sind die `Desktop*`-Tools der richtige Einstieg.
