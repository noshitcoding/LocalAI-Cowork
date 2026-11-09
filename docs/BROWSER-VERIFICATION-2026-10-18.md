# Browserintegration: geprüfter Umfang am 18. Oktober 2026

Aktuelle V2-Quellen, Windows x64 und Ubuntu 24.04 unter WSL, jeweils Chrome for
Testing 155.0.8059.84 und Playwright-Firefox Revision 1543 mit Playwright 1.63.0.
Dies ist eine Entwicklungsprüfung, keine signierte Release- oder Storeabnahme.

| Prüfung | Ergebnis |
| --- | --- |
| Frontend | 73 Testdateien, 392 Tests bestanden; TypeScript und Produktionsbuild erfolgreich |
| Verträge | 645 generierte Verträge; Rust-Schema, OpenAPI, TypeScript und Validatoren stimmen überein |
| Rust-Kern | Toolbox: 11 Tests bestanden; der laufzeitabhängige Test wurde zusätzlich ausdrücklich ausgeführt |
| Native Host | Drei Tests für geordnete Chunks, große Unicode-Nachrichten und atomare Manifest-Ersetzung bestanden |
| Betriebssystemtransport | Echte Named Pipe unter Windows und echter Unix-Socket unter Linux mit dem aktuellen Benutzer geprüft |
| Native Quellen | Windows-Checks für Daemon, Geräteagent, Host und Tauri-Shell erfolgreich; Toolbox/Host zusätzlich unter Linux geprüft |
| Paketiervorbereitung | 20 Tests mit einem plattformabhängigen Skip sowie 12 Release-Tests bestanden |
| Oberfläche | 1440 px und 390 px geprüft, keine horizontalen Überläufe oder Seitenfehler; Adresse bleibt beim Tippen trotz Livebildern erhalten; Mausrad und Tastatur gelangen an die Browsersitzung |

## Tatsächliche Browser

Chrome und Firefox bestanden unter Windows und Linux Navigation, Formulare,
Auswahlfelder, Maus-/Tastatureingaben, Scrollen, Ansichtsgrößen, Livebilder, Tabs,
Verlauf, Screenshots, Annotationen und Entwicklerbeobachtungen. Passwortwerte
werden nicht als DOM-Inhalt zurückgegeben. Cookies bleiben nach Schließen und
erneutem Öffnen desselben privaten Profils erhalten. Ein veralteter Dokumentstand
wird zurückgewiesen.

Der ignorierte Rust-Laufzeittest wurde für beide Engines auf beiden Systemen
ausdrücklich gestartet. Er verwendet das echte Toolbox-Objekt, manuelle Eingaben,
die Rückgabe an die KI-Steuerung, die exakte Vorschau-/Freigabe-/Jobausführung,
Originalquittungen und anschließend dasselbe erneut geöffnete Projektprofil.

## Tatsächliche Erweiterungen

Die gebauten Erweiterungen wurden unter beiden Betriebssystemen tatsächlich in
Chrome und Firefox geladen. DOM-Zugriff, Ausfüllen, Klicks und Cookies wurden auf
lokalen Testseiten geprüft. Nicht freigegebene Tabs und veraltete Dokumentstände
wurden abgewiesen; Widerruf entfernt die Tabfreigaben.

Zusätzlich zur deterministischen nativen Testgegenstelle wurde die vollständige
reale Kette unter beiden Betriebssystemen und mit beiden Browsern geprüft:
Erweiterung → `connectNative` → Rust-Host über stdio → benutzerbeschränkte Named
Pipe beziehungsweise Unix-Socket → Toolbox-Vorschau/Freigabe/Job → Browserseite.
Dabei waren vorhandene Testcookies verwendbar und nur der freigegebene Tab wurde
an Cowork übermittelt. Jede Browseraktion lief durch den tatsächlichen Jobpfad;
ein wiederholter Jobaufruf lieferte dieselbe Quittung ohne erneute Ausführung.

Diese Prüfung verwendet frische Browserprofile, vorab erteilte Webseitenrechte
und einen zufälligen temporären Native-Host-Namen. Seine Testregistrierungen
werden anschließend entfernt. Produktionsregistrierungen und vorhandene
Benutzerprofile werden nicht verändert. Storeinstallation und Produktionssignierung
sind damit weiterhin nicht qualifiziert.

## Lokale Modellaufgabe

Der vorhandene lokale Ollama-Dienst mit `qwen3:4b` erzeugte für die beobachtete
Testseite die Aktionen zum Eintragen von „Browser acceptance passed“ und zum
anschließenden Speichern. Diese tatsächlich erzeugten Aktionen bestanden:

- verwaltetes Chrome und Firefox über den Toolbox-Freigabe-/Jobpfad unter Windows
  und Linux;
- Chrome- und Firefox-Erweiterung auf echten Browserseiten unter Windows und
  Linux, sowohl mit deterministischer Testgegenstelle als auch über die vollständige
  reale Native-Messaging-/Toolbox-Kette.

Der erzeugte Plan kann über `COWORK_BROWSER_TEST_MODEL_ACTIONS` an die jeweiligen
Laufzeittests übergeben werden. Dieser Nachweis umfasst die lokale Modellplanung
und die tatsächlichen Browseraktionen; er ist keine Abnahme aller Modellanbieter
oder der vollständigen Chatoberfläche mit einer externen Modellverbindung.

## Verbleibende Auslieferungsgrenzen

- Keine bestehende App-Installation aktualisiert und keine dauerhafte
  Produktionsregistrierung angelegt; isolierte Testregistrierungen wurden entfernt.
- Keine Store-Veröffentlichung und keine Firefox-Produktionssignierung.
- Keine vollständige reale NSIS-/DEB-/AppImage-Installation oder Matrix aller
  Snap-/Flatpak-Portale qualifiziert.
- Clippy läuft mit neun bestehenden Hinweisen in älteren Toolbox-Bereichen;
  `-D warnings` besteht deshalb nicht. Die neuen Browsermodule und der Host
  erzeugten nach Bereinigung keine zusätzlichen Clippy-Hinweise.

Einrichtung und reproduzierbare Pakete: [Browserintegration](BROWSER-INTEGRATION.md).
