# Alpha 5 installieren und einrichten

## Desktop

Bei einem Upgrade die bisherige App schließen und das Gerät vor dem ersten Alpha-5-Start neu starten, damit die ältere lokale Instanz ihren Port freigibt. Eine bereits laufende ältere Instanz am lokalen Port wird von Alpha 5 nicht ersetzt.

1. Windows: den mitgelieferten `x64-setup.exe`-Installer öffnen. Linux: das `.deb` installieren oder die AppImage entsprechend den Regeln der eigenen Distribution starten.
2. In der App **Lokalen Arbeitsbereich öffnen** wählen. Der private V2-Datenordner und die eigene lokale Instanz werden vom Desktop verwaltet.
3. Unter **Werkzeuge → Werkzeuge einrichten** die private Laufzeit starten oder vorhandene Programme auswählen. Die App zeigt Fortschritt und Fehler und lädt die fertige Konfiguration sofort. Eine Server- oder Team-Anmeldung ersetzt keine lokale Werkzeugfreigabe.
4. Eine eigene Modellverbindung einrichten. Für Teams mit Teamkonto am eigenen Server anmelden und unter **Einstellungen → LiteLLM-Verbindung** den eigenen Team-Schlüssel speichern.

## Eigener Server

Das Archiv enthält das eigenständige Linux-Serverbundle, das Webfrontend, Start-/Installationshelfer und den Quellcode mit Docker-Konfigurationen. Die vorhandene `tools/server/README.md` beschreibt Benutzerinstallation und Reverse Proxy. Für Teamwerkzeuge das getrennte Image `deploy/v2/Dockerfile.tools` auf dem eigenen Server bauen und seine Container-/Arbeitsbereichskonfiguration auswählen. Kein Installationshelfer startet fremde VMs oder übernimmt bestehende Benutzerprofile.

Die Serveradresse und die zugelassenen Ursprünge müssen zu der eigenen HTTPS-Bereitstellung passen. Beim ersten Start erzeugt die Instanz ihre eigenen privaten Betreiber- und Bootstrapzugänge. Diese werden nicht in öffentlichen Quell-/Installationspaketen mitgeliefert.

## Passkeys, Android-Push und Fleet

Mit dem privaten Betreiberzugang im Arbeitsbereich **Geräte & Sync → Gerätedienste einrichten** öffnen:

* **Passkeys:** exakte öffentliche HTTPS-Adresse ohne abschließenden Pfad und deren Hostname als RP-ID speichern. Für lokale Entwicklung ist `http://localhost:PORT` möglich. Im Team unter **Geräte** den persönlichen Passkey direkt am konfigurierten Webursprung registrieren.
* **Push:** das eigene Firebase-Servicekonto als JSON auswählen und speichern. **Konfiguration prüfen** führt eine OAuth-Anmeldung aus. In der bestehenden Android-App beziehungsweise in der Team-Geräteansicht den eigenen FCM-Gerätetoken registrieren. Anbieterannahme ist kein Nachweis einer tatsächlich auf dem Telefon angezeigten Nachricht.
* **Fleet:** eigene Organisations-/Instanz-UUID und den gemeinsamen Fleet-Schlüssel speichern. Dieser Verifikator akzeptiert die begrenzten Tickets des eigenen Fleet-Ausstellers; er erzeugt keine neuen Identitäten durch Übernahme alter V1-Benutzer.

Leere neue Zugangsfelder erhalten den vorhandenen Zugang. Die ausdrücklichen Entfernen-Schalter deaktivieren die jeweilige Funktion und entfernen den gespeicherten Zugang. Gespeicherte Einstellungen werden verschlüsselt und gelten sofort sowie nach Neustart.

## Geräteagent

Im Team unter **Geräte** Gerät, Art, Pool, Capability-Namen und Fernsteuerungsfreigabe auswählen und koppeln. **Geräteprofil speichern** enthält ausschließlich den neu ausgestellten Gerätezugang und seine Registrierung. Das Profil auf das eigene Gerät übertragen und mit `tools/device_agent/configure.py` in einen neuen privaten Agentordner übernehmen. Die zusätzliche [V2-Agent-Auslieferung](DEVICE-AGENT-V2.md) enthält einen eigenständigen Windows-/Linux-Agenten; V1-Daemon und V1-IPC werden nicht mehr benötigt. Eigene Modellverbindung, Werkzeuge und deren Zugänge wählt der Gerätebetreiber. Der Agent führt einen Lauf gleichzeitig aus. Windows-Desktopstreaming benötigt eine interaktive Sitzung; Linux-Desktopstreaming ist noch offen.

## Offline-Auftrag

Vor dem Trennen der Cowork-Verbindung eine signierte, begrenzte Offlinefreigabe für das eigene Gerät ausstellen. Eigene Modellzugänge und die erlaubten Gerätewerkzeuge unter **Offline-Ausführung** konfigurieren. GitHub-/MCP-Zugänge werden am Gerät eingegeben; Team-Server-Schlüssel werden nicht exportiert. Dateien gezielt in den Offline-Arbeitsbereich übernehmen, den Lauf starten und konkrete Werkzeuge freigeben. Nach Wiederverbindung Bericht und Artefakte ausdrücklich abgleichen.

## MCP-Anmeldung und Rückfragen

Für OAuth zuerst den lokalen OS-Schlüsselspeicher bestätigen bzw. am Server den exakten öffentlichen HTTPS-Ursprung einrichten. Beim Anbieter einen Client mit dem in der Oberfläche angezeigten Redirect registrieren. Im jeweiligen MCP-Eintrag **OAuth-Anmeldung → Anbieter prüfen** öffnen, Client-ID, Scopes und gegebenenfalls das Client-Secret ausschließlich im privaten Eingabefeld speichern. Die tatsächliche Browseranmeldung öffnen und danach im ursprünglichen angemeldeten Kontext **Originale Anmeldung abschließen** wählen. Die ausführlichen Schritte und der Umgang mit abgelaufenen oder unbekannten Tokenantworten stehen in [MCP im Auftragsablauf](MCP-PRODUCT.md).

Im Auftrag unter **MCP-Tasks und Eingaben** das Originalergebnis, einen Task-Abbruch und menschliche Rückfragen bedienen. Sampling verlangt zuerst die Zustimmung zu genau einem Aufruf des aktuellen Modells und anschließend eine separate Freigabe der tatsächlich erzeugten Antwort. Geräte zeigen diese Eingaben im vorhandenen Anfragenbereich; für stdio die eigenen ausgewählten Programm-/Pythonpfade konfigurieren. Der [Geräteagent-Leitfaden](DEVICE-AGENT-V2.md) beschreibt dessen privaten Originalbelegspeicher und die installierten Diagnosebefehle. Ein verlorenes ACK oder ein unbekannter Effekt berechtigt nicht zum automatischen Wiederholen des Werkzeugs, Modells oder Refreshs.

Die Software ist als Alpha ausgeliefert. Die Builds und wenige Demoabläufe werden dokumentiert; eine umfangreiche Produktionsqualifizierung oder Signierung ist nicht Bestandteil dieses Pakets.

Die aktuelle Offline-MCP-Oberfläche erhält Vorgangskennung, Aktion, Revision und
Hash der ursprünglichen Eingabe nach einem Neuladen. **Ursprüngliche
Freigabequittung abrufen** bzw. **Ursprüngliche Abbruchquittung abrufen** liest
ausschließlich den ursprünglichen Beleg. Ist noch kein Beleg vorhanden, müssen
dieselben ursprünglichen Eingaben ausdrücklich erneut übergeben werden.
Formular- und Modelltexte werden für diese Wiederaufnahme nicht im Browser
gespeichert. Nach einem Neuladen sind sie bei Bedarf erneut unverändert
einzugeben. Gleichzeitig geöffnete Ansichten werden über Web Locks
koordiniert; ohne diese Browserfunktion wird keine neue Freigabe versendet.
