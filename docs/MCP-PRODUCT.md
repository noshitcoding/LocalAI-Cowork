# MCP im Auftragsablauf

Quellstand 12. Oktober 2026: Die aktuelle persönliche Oberfläche ergänzt
HTTP-Katalogentdeckung, nativ bestätigte Bearer-Einrichtung und stdio-Fabriken
mit Katalogprüfung sowie ausdrücklicher Verbindungstrennung. Die Aufrufansicht
besitzt einen öffentlichen, beleggebundenen Reconciliation-Pfad. Diese Funktionen
stehen in `McpTools.tsx`, `native_mcp.rs` und den aktuellen MCP-Routen. Die darunter
beschriebene Abnahme vom 8. Oktober ist historisch; ihre damaligen offenen
Produktpfade sind kein aktuelles Fehlfeature-Inventar. Reale Dienste, native
Dialoge und der vollständige aktuelle Werkzeugbetrieb bleiben separat abzunehmen.
Der additive MCP63-Quellstand vom 13. Oktober ergänzt OAuth/Refresh, originale
asynchrone Task-ACKs und bestätigte Sampling-/Formularrückfragen. Die folgenden
Einrichtungsschritte beschreiben die tatsächlichen aktuellen Produktoberflächen.
Die älteren Abschnitte darunter bleiben historische Belege, keine aktuelle Restliste.
Die laufende Geräte-/Offline-Integration, aktuelle Lieferartefakte und reale
Anbieter-/Geräteabnahmen haben jeweils einen eigenen Abschlussnachweis.

## Aktuelle OAuth-Einrichtung

1. Lokaler Desktop: unter „Lokale Sicherheit“ den OS-Schlüsselspeicher bestätigen.
   Den MCP-Server im Projekt mit seinem tatsächlichen Katalog einrichten bzw.
   „Anbieter prüfen“ ausführen. Die native OAuth-Verwaltung ist nur über den
   verbundenen lokalen Desktop und dessen private IPC verfügbar.
2. Teamserver: der Betreiber setzt den **exakten öffentlichen HTTPS-Ursprung**
   `COWORK_PUBLIC_ORIGIN`. Die Team-/persönliche Oberfläche zeigt den daraus
   erzeugten Redirect `https://HOST/api/v1/mcp/oauth/callback`. TLS, Reverse Proxy
   und diese tatsächliche Route müssen erreichbar sein; Hostname/Redirect dürfen
   nicht aus einem fremden Request-Header übernommen werden.
3. Beim ausgewählten Authorization Server einen Client registrieren. Für den
   lokalen Desktop lautet der Redirect exakt
   `http://127.0.0.1:43821/mcp/oauth/callback`; für Serverzugänge gilt der angezeigte
   HTTPS-Redirect. Client-ID, erlaubte Redirects, unterstützte Token-Authmethode
   und benötigte Scopes müssen zur Registrierung passen. Es gibt keine erfundene
   dynamische Clientregistrierung oder pauschale Übernahme fremder Anbieter.
4. Im jeweiligen MCP-Eintrag „OAuth-Anmeldung“ öffnen, „Anbieter prüfen“ wählen,
   den tatsächlich angekündigten Authorization Server auswählen und Client-ID,
   gegebenenfalls Client-Secret sowie die kleinsten nötigen Scopes eintragen.
   Geheimnisse gehören ausschließlich in diese privaten Eingaben. Sie gehören
   weder in Laufaufträge, Artefakte, Browser-URLs noch in diese Dokumentation.
5. Teamzugänge verlangen eine ausdrückliche gemeinsame Teamfreigabe. Persönliche
   Serverzugänge werden im privaten Projekt unter „Eigene MCP-Verbindungen“
   eingerichtet; sie übernehmen keine Owner-, Team- oder Geräteschlüssel.
6. Konfiguration speichern, Anmeldung vorbereiten und die tatsächliche
   Anbieteranmeldung öffnen. Der Browser-Callback erfasst nur die originale
   Antwort zu State/Issuer/Redirect. Anschließend im **ursprünglichen angemeldeten
   Kontext** „Originale Anmeldung abschließen“ wählen. Der Callback meldet niemanden
   an und führt selbst keinen Token-POST aus.

Authorization Code und Refresh werden nach einer dauerhaften Originalabsicht
genau einmal gesendet. Eine unbekannte Tokenantwort wird niemals automatisch
wiederholt. Der Zustand „unknown“ verlangt die Prüfung/Widerruf beim Anbieter
und eine ausdrücklich bestätigte neue Konfigurationsrevision; eine alte
Anmeldung oder ein verlorener Refresh-Token wird dabei nicht als neue Autorität
übernommen. Abgelaufene Browserflows müssen ausdrücklich neu vorbereitet werden.
Ein manueller Bearer bzw. eine bestehende stdio-Fabrik bleibt eine eigene
Konfigurationsart. Der Anbieter muss PKCE S256 und das verwendete OAuth-Profil
tatsächlich unterstützen.

## Tasks, menschliche Eingaben und Originalbelege

Tasks werden nur nach der tatsächlich ausgehandelten Version `2025-11-25` und
den angekündigten Serverfähigkeiten angefordert. Ein `taskSupport: required`
Werkzeug wird nicht durch einen scheinbar synchronen Erfolgsbeleg ersetzt.
Die Task-ID stammt ausschließlich aus dem originalen `tools/call`-ACK. Der
Modell-/Workflowlauf wartet auf `tasks/result`, statt das ursprüngliche Werkzeug
nochmals aufzurufen. Auch `failed`/`cancelled` verlangen das originale
Ergebnis bzw. JSON-RPC-Fehler mit übereinstimmender related-task-ID.

Im Auftrag zeigt „MCP-Tasks und Eingaben“ Status, Originalergebnis und
Task-Abbruchanfrage. Eine bereits laufende Ergebnisoperation wird nicht für
einen neuen Werkzeugaufruf ersetzt; Abbruch bleibt an den ursprünglichen Task
gebunden. Abbruch, Ablauf, HTTP-Abbruch und Fehler beweisen **keine** fehlenden
Änderungen im entfernten System. Unbekannte Wirkungen sperren automatische
Wiederholung und Wiederaufnahme, bis echte Originalbelege eine Klärung erlauben.

Formulare zeigen die unveränderte Serveranfrage und ihre begrenzten skalaren/
Enum-Felder. Werte nach eigener Prüfung bestätigen, ablehnen oder die Eingabe
abbrechen; keine Passwörter, API-Schlüssel oder Zahlungsdaten eingeben. URL-
Elicitation, externe Schema-Referenzen und beliebige zusammengesetzte Schemas
werden nicht als unterstützte Formulare angekündigt.

Sampling verlangt zwei ausdrückliche Entscheidungen: zuerst die geprüften,
bearbeitbaren Anfragetexte und genau einen Aufruf des **ursprünglichen** aktuellen
Modells/der Workflowrolle bestätigen; danach die tatsächlich generierte Antwort
prüfen und genau diese Antwort freigeben oder zurückhalten. Der MCP-Server kann
kein anderes Modell, Credential, fremden Kontext oder zusätzliche Werkzeuge
auswählen. Originale Modell-/Kontext-/Schrittgrenzen bleiben wirksam; dafür wird
kein internes Team-Token- oder Geldbudget eingeführt. Verlorene Entscheidungen
werden anhand ihrer ursprünglichen Quittung geklärt, niemals als neuer Modellaufruf.

Task-ACKs, RPC-Antworten, Anfragen, Entscheidungen, Modellantworten und einmalige
Antwortversandabsichten werden verschlüsselt und an ihren Originalkontext gebunden.
KMS-/native Inventar- und Rewrappfade erhalten ihre originalen AAD-/Fingerprint-
Schlüssel. Ein positiver privater stdio-Prozessabschluss hält zusätzlich den
verschlüsselten Original-Sitzungsstand und den tatsächlichen Stop-/Entfernungsbeleg
fest. Dieser Beleg ersetzt keinen fehlenden Werkzeug-/Modellabschluss.

Stand: 8. Oktober 2026. Die persönliche V2-Vorschau verbindet das vorhandene
MCP-Journal jetzt mit dem normalen Modell-/Werkzeuglauf und der gemeinsamen
Web-/Desktop-Oberfläche. Dies ist eine begrenzte Streamable-HTTP-Integration;
M4 und der Gesamtplan M0–M10 sind weiterhin nicht abgenommen.

## Einrichten und auswählen

1. Im Desktop unter „Lokale Sicherheit“ den nativen Schlüsselspeicher einrichten.
   Auch ein Server ohne Bearer verwendet verschlüsselte Sitzungs-Checkpoints.
2. Im Auftrag „Externe Werkzeuge“ öffnen und einen MCP-Server hinzufügen.
3. Namen und Endpoint angeben und ein JSON-Array der bekannten `tools/list`-
   Definitionen importieren. Die Definitionen müssen zum tatsächlichen Server passen.
4. Werkzeuge für das Projekt zulassen und die Konfiguration speichern.
5. Aus diesem Angebot ausdrücklich Werkzeuge für den nächsten Lauf auswählen.
   Projektfreigabe allein fügt keinem Lauf Werkzeuge hinzu.
6. Auftrag starten und die konkrete Aktion entsprechend dem Zugriffsmodus freigeben.

Die Konfiguration führt keine Netzwerkanfrage aus. Der Server validiert Katalog,
Schemas, Endpoint und Auswahl. Erst der zugelassene Modellaufruf initialisiert die
Verbindung und vergleicht den tatsächlichen Kataloghash. Abweichungen blockieren
die Ausführung. Automatische Katalogentdeckung in der Einrichtung ist noch offen.

Unterstützte Zieladressen sind HTTPS und Loopback-HTTP ohne eingebettete
Zugangsdaten, Query oder Fragment. Es werden keine lokalen Programme gestartet.
Die Oberfläche richtet noch keine Bearer-Credentials ein. Bereits über den
privaten nativen Dienst angelegte, exakt gebundene Credentials bleiben nutzbar;
ein fehlender Schlüssel oder eine widerrufene Credential führt nicht zu einem
anonymen Ersatzversuch.

## Fachliche und technische Bindung

`CreateRun.mcp_tools` enthält ausschließlich Server-ID, beobachtete Serverrevision
und Werkzeugname. Der Client darf weder die auszuführende Definition noch einen
Endpoint oder Zugangsdaten in diese Auswahl einschleusen. Leere/historische
Auswahl verändert die bisherigen serialisierten Startanfragen nicht.

Preflight löst diese Referenzen im Projekt des Auftrags auf. Es prüft die
explizite Projektauswahl und die native Schlüsselverfügbarkeit. Der erzeugte
Modellkatalog sowie die Kontext-/Budgetprüfung verwenden dieselben eingefrorenen
Definitionen. Der Modellname des Werkzeugs ist ein stabiler, servergebundener Alias.

Die Starttransaktion prüft die Bindungen nochmals und hält dieselben Server- und
Credential-Schreibsperren wie die Aufrufzulassung. Eine Änderung nach Preflight
wird dadurch nicht still übernommen. Revisionskonflikte lassen den Auftragsstand
unverändert. Der Dispatcher prüft die aktuelle Bindung erneut vor Remote-I/O.
Ein bereits angenommener Lauf wechselt bei Entzug nicht auf eine andere Revision.

Die vier Zugriffsmodi gelten unverändert. Alle MCP-Werkzeuge gelten zunächst als
ändernd; `readOnlyHint` und `idempotentHint` eines fremden Servers sind kein
Sicherheitsnachweis. Im Standardmodus erfolgt vor Initialisierung und Werkzeug-POST
eine konkrete Freigabe. „Nur lesen“ verweigert den Aufruf ohne Verbindung.
Rechte innerhalb des entfernten Dienstes müssen zusätzlich dort begrenzt werden;
die lokale Freigabe ist keine Sandbox für den entfernten Server.

Der normale Worker verwendet den bestehenden Dispatcher mit dauerhafter
Zulassung und verschlüsselten Checkpoints. Werkzeugergebnis, Entfernung des
offenen Schritts und neuer Sitzungsstand werden atomar gespeichert. Die
Modellschleife schreibt keinen zweiten Ergebnisbeleg. Strukturierte externe
Ergebnisse bleiben im Aufrufjournal; sie sind nicht automatisch Dateiartefakte.

Nach unklarem Ausgang bleibt der Lauf unterbrochen. Es gibt keinen automatischen
Werkzeug-Replay. Aufruf und geschützte Antwortbelege sind im Auftrag sichtbar.
Fortsetzen und Abbrechen bleiben bei ungeklärten unterbrochenen Aufrufen auch in
der Oberfläche gesperrt. Die öffentliche Oberfläche zur belegten Klärung ist noch nicht implementiert;
der private Reconciliation-Vertrag bleibt unverändert. Ein HTTP-Abbruch beweist
nicht, dass eine entfernte Aktion beendet wurde.

## API und Oberfläche

| Vertrag | Verhalten |
|---|---|
| `GET /api/v2/projects/{project}/mcp-servers` | Validierte Konfigurationen des Projekts; unbekanntes Projekt liefert 404 |
| `POST /api/v2/projects/{project}/mcp-servers` | Revisionierte Konfiguration mit Idempotenzschlüssel, ohne Remote-I/O |
| `POST /api/v2/preflight` und `/runs` | Explizite Werkzeugreferenzen, eingefrorene Definitionen und erneute Startprüfung |
| `GET /api/v2/runs/{run}/mcp-attempts` | Autorisierte Aufrufbelege; dieselben Belege stehen in RunDetail und Export |

Die Konfigurationsrouten gehören zur Owner-API des lokalen Daemons. Sie prüfen
Bearer und zulässigen Origin und sind keine Modellwerkzeuge. Sie stellen keine
Enterprise-Rollen- oder Mandantenverwaltung dar. Rohschlüssel werden nicht angenommen.
JSON-Schema, OpenAPI, TypeScript und Validatoren entstehen aus Rust; insgesamt
100 Vertragsdefinitionen. Bestehende Datenbankschema-Version 15, Ereignisschema 7
und Exportformat 9 bleiben unverändert.

Ein erneutes Senden nach unbestätigter Konfigurationsspeicherung verwendet die
identische Anfrage. Alte Belege können einen deaktivierten Server nicht
reaktivieren. Nach dem Schließen wird der aktuelle Projektstand erneut gelesen.
Änderungen ersetzen eine alte Lauf-Auswahl nicht automatisch. Der Nutzer sieht
einen Konflikthinweis und muss neu auswählen. Ein neuer Auftrag beginnt ohne
MCP-Auswahl; es gibt keine automatische Rechteübernahme aus früheren Läufen.

Deutsch/Englisch verwenden gemeinsame Übersetzungsschlüssel. Freigaben zeigen
Werkzeugname, Ziel und konkrete Argumente. Interne IDs und Belege stehen in
aufklappbaren Diagnosen. Die Aufrufansicht verwendet die vorhandene RunDetail-
Abfrage, keine zusätzliche Abfrage pro Ereignisrevision.

## Prüfung und verbleibende Arbeit

Sechs zusätzliche Runtime-Prüfungen verbinden einen synthetischen
OpenAI-kompatiblen Streaming-Anbieter mit einem zustandsbehafteten lokalen
MCP-Testserver. Dieser schreibt bei Werkzeugaufrufen tatsächlich eine Datei.
Geprüft sind Standardfreigabe über DB-/Runtime-Neustart, einzelne Ausführung und
Rückgabe an das Modell, Nur-Lesen ohne Netzwerk, Entzug während Freigabewartezeit,
Fehler nach bereits erfolgter Änderung ohne Wiederholung, fremde/doppelte/veraltete
Auswahl, erneute Startprüfung sowie fehlende Schlüssel ohne Ersatzanlage.

Zwei zusätzliche HTTP-Tests prüfen Konfigurationsimport, Origin-Sperre,
Projektzuordnung, alte Idempotenzbelege, Revisionen, unerlaubte Credential-Felder,
Remote-Schema-Verweise und unbekannte Objekte. Der gemeinsame SQLite-/PostgreSQL-
Vertrag prüft zusätzlich Serverlisten und ausgewählte Bindungen. Zwei zusätzliche
Clienttests prüfen Referenzgrenzen und fehlende automatische Mutationswiederholung.
Aktuelle Plattformbelege stehen in [VERIFY.md](VERIFY.md).

Offen bleiben Katalogentdeckung und Credential-Einrichtung in der Oberfläche,
OAuth, explizite Neuverbindung und Sitzungsbeendigung, öffentliche belegte
Reconciliation, stdio und weitere Protokollfähigkeiten, echte MCP-Serverabnahme,
Netzwerk-Zielrichtlinien, Enterprise-Identität und Server-KMS. Die Browserprüfung
mit synthetischen API-Antworten ersetzt keine reale native Dialog- oder
MCP-Produktabnahme auf beiden Desktopplattformen. Es erfolgte kein Neustart der
bestehenden Vorschau, keine Veröffentlichung und keine Umstellung von V1.

Der neue Offline-Taskpfad im V2-Quellstand verwendet ausschließlich die signierte
Gerätefreigabe und die lokal eingerichtete MCP-Verbindung. In der nativen
Offline-Ausführungsansicht werden ursprüngliche Task-ACKs, Status-/Ergebnisbelege,
Abbruchanfragen und lokale Eingabeformulare angezeigt. Sampling verwendet genau
den freigegebenen Geräte-Modellendpunkt und denselben vorhandenen Zähler von
höchstens acht Modellaufrufen. Erst nach Prüfung der Anfrage und einer weiteren
ausdrücklichen Prüfung der erzeugten Antwort wird diese an den MCP-Server
freigegeben. Die Originale sind im separaten OfflineDevice-Keybereich versiegelt;
es werden keine Team-, NativeState- oder OAuth-Schlüssel übernommen.

Bei verlorenen Antworten ruft die Oberfläche dieselbe ursprüngliche
Freigabequittung ab. Ein bereits versendetes `tools/call` wird nicht wiederholt;
unbekannte Wirkungen bleiben unbekannt. Vollständige Eingaben sind in zwei
Originaldatensätze umfassenden Schlüsselseiten erreichbar. Ergebnisdownloads
erhalten die vollständigen Original-JSON-Bytes und werden anhand Größe und SHA256
geprüft. Ein Task-Fehler oder Abbruch bestätigt keine physische Wirkungslosigkeit.
Reale MCP-Task-, Eingabe-, Sampling-, Neustart- und Paketabnahmen für diesen neuen
Offline-Schnitt bleiben gesonderte Produktionsnachweise.
