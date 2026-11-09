# Store-Einreichung

Die lokalen ZIPs werden nicht automatisch veröffentlicht oder signiert. Chrome
Store und AMO erfordern die jeweiligen Entwicklerkonten. Temporäre Firefox-
Installation ersetzt keine signierte Produktions-XPI.

## Reproduzierbare Pakete

`npm run browser:build` und `npm run browser:package` erzeugen getrennte ZIPs
unter `dist/browser-extensions` mit festen ZIP-Zeitstempeln und SHA-256-Quittung.
Selenium und Playwright in den Testskripten sind keine Erweiterungsabhängigkeiten.
Die Erweiterung lädt keinen entfernten ausführbaren Code.

Die öffentliche Entwicklungsidentität steht in `identity.json`. Vor der ersten
Chrome-Store-Auslieferung die vom Store zugewiesene Erweiterungsidentität und
ihren öffentlichen Schlüssel hier festlegen und den Native Host gemeinsam neu
bauen. Eine bestehende Store-Identität bei Updates beibehalten. Firefox verwendet
die feste Gecko-ID aus derselben Datei; die AMO-Verfügbarkeit dieser ID muss das
Entwicklerkonto bestätigen. Keine privaten Signaturschlüssel in diese Datei
eintragen. Native-Manifeste erlauben ausschließlich diese konkreten IDs.

## Beschreibung und Berechtigungen

Name: **LocalAI Cowork Browser**

Kurzbeschreibung: **Ausgewählte Tabs mit der eigenen lokalen Cowork-App verbinden.**

- `nativeMessaging`: lokale Kommunikation mit dem installierten Cowork-Host.
- `tabs`: Tabauswahl und Metadaten ausdrücklich freigegebener Tabs.
- `scripting`: feste DOM-Aktionen auf freigegebenen HTTP(S)-Webseiten.
- `activeTab`: ausdrücklicher Benutzerzugriff auf den aktuellen Tab.
- Optionaler Zugriff auf alle URLs: erst beim Verbinden angefragt; die interne
  Tabfreigabe gilt zusätzlich. Interne Seiten, Dateien und private Fenster sind
  von der Implementierung ausgeschlossen.

Die Erweiterung besitzt keine Telemetrie, keinen eigenen externen Modellzugang
und keinen automatischen Zugriff auf andere Tabs. Freigegebene Seiteninhalte und
Aktionsergebnisse gelangen an die gewählte lokale Cowork-Instanz. Deren vom Nutzer
gewählte Modell-/Serverkonfiguration kann diese Inhalte weiterverarbeiten.
Entsprechend nennt das Firefox-Manifest Website-Inhalte und Website-Aktivitäten
als erforderliche Datenkategorien. Vor Veröffentlichung eine erreichbare
Datenschutzerklärung, Supportadresse und Store-Screenshots ergänzen und diese
Angaben mit der tatsächlichen Cowork-Datenverarbeitung abgleichen.

## Review-Schritte

1. Aktuelle native App oder den separat gelieferten Host mit `--register` einrichten.
2. Cowork-Browserbereich auf **Installierter Browser** stellen, Instanz suchen.
3. Zwei normale Testseiten öffnen, im Popup nur eine davon freigeben.
4. Freigegebenes Formular in Cowork auslesen und ausfüllen; die andere Seite muss
   unerreichbar bleiben. Cookie-/Anmeldesitzung wird im Browser weiterverwendet.
5. **Zugriff beenden** und Browserneustart prüfen: beide entziehen die Freigaben.
6. Hostausfall und veraltete Dokumentkennung prüfen: keine automatische Wiederholung.
