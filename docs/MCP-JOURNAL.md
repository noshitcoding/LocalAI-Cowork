# Dauerhafte MCP-Aufrufe

Stand: 8. Oktober 2026. Implementiert sind ein privates Serverregister, das
transaktionale Aufrufjournal und ein gemeinsamer Runtime-Einstieg. Verschlüsselte
Sitzungs-Checkpoints erlauben inzwischen die Wiederaufnahme bestätigter Verbindungen;
siehe [MCP-SESSIONS.md](MCP-SESSIONS.md). Native Schlüssel und verschlüsselte Credentials
werden über den ergänzten Dienst in [MCP-SECRETS.md](MCP-SECRETS.md) aufgelöst.
Modellschleife, Einrichtungs-/Daemon-Verwaltungsrouten und Produktoberfläche sind
noch nicht angeschlossen. V1 und bestehende
Vorschauprozesse werden durch diesen Baustein nicht umgestellt.

## Konfiguration und Rechte

`configure_mcp_server` bindet einen Server an genau ein Projekt. Die normalisierte
Adresse, der vollständige geprüfte Katalog, ausgewählte Werkzeuge, Aktivierung und
optionale Credential-Referenzen besitzen eine Revision und Inhaltsprüfsummen.
Credential-Referenzen sind Kennungen; Geheimnisse dürfen nicht in diese Datensätze.
Die private native Auflösung sowie revisionierte Rotation und endgültiger Entzug
sind implementiert. Authentifizierte Verwaltungsoberflächen und Server-KMS bleiben offen.

Änderungen verlangen eine Basisrevision und einen Idempotenzschlüssel. Konfiguration
und Änderungsbeleg werden in derselben Transaktion gespeichert. Die Wiederholung
einer früheren Aktivierung liefert deren alten Beleg, aktiviert einen inzwischen
entzogenen Server aber nicht erneut. Projektfremde Abfragen werden abgewiesen.

Ein Lauf friert Serverrevision, Endpoint, Kataloghash, Definition und einen stabilen
Werkzeugalias ein. Modellargumente wählen weder Endpoint noch Credentials. Alle
ausgewählten MCP-Werkzeuge gelten als potenziell schreibend, auch bei
`readOnlyHint: true` oder `idempotentHint: true`. Sie müssen ausdrücklich in den
Auftragsrechten stehen und durchlaufen dieselbe Policy und exakte Freigabe wie
andere Werkzeuge. „Nur lesen“ sperrt sie; die beiden Bestätigungsmodi verlangen
eine passende Entscheidung. Organisationsrichtlinien fehlen weiterhin im Produkt.

## Zulassung, Dispatch und Abschluss

1. Die verbundene Sitzung prüft Katalog und Argumente. Sie erzeugt vor dem
   Datenbankabschluss die JSON-RPC-ID und einen nicht klonbaren vorbereiteten Aufruf.
2. `admit_mcp` prüft Lease, ausstehenden Werkzeugschritt, Rechte, Freigabe,
   Zeit-/Schrittbudget, eingefrorene Bindung und aktuelle Serverrevision.
3. Journal, Unsicherheitsmarker, Ereignis und Schreibbesitz werden gemeinsam
   gespeichert. Die Konfigurationszeile wird bis zum Abschluss gegen gleichzeitigen
   Entzug gesperrt. Ein fehlgeschlagener Abschluss erteilt keine Sendeberechtigung.
4. Nur die erste bestätigte Zulassung gibt den ursprünglichen vorbereiteten Aufruf
   zurück. Parallele oder spätere Wiederholungen liefern ausschließlich den
   gespeicherten Versuch. Auch eine verlorene Datenbankbestätigung erzeugt keinen
   rekonstruierten Aufruf.
5. `Runtime::dispatch_mcp_tool` prüft die Zulassung unmittelbar vor Netzwerkzugriff
   erneut und beobachtet währenddessen Abbruch, Laufbudget und Worker-Lease.
6. Eine gültige Erfolgsantwort wird mit Hash, Werkzeugnachricht, Schrittzähler und
   Ereignis atomar abgeschlossen. Derselbe Abschluss kann idempotent gelesen werden.

Der Hash bindet Lauf und eingefrorene Konfiguration sowie Werkzeug und Argumente.
Zusätzlich bindet der Transporthash Endpoint, Protokoll, Katalog, Aufruf, lokale
Verbindungskennung und tatsächliche JSON-RPC-ID. Die Verbindungskennung ist ein
lokaler Zufallswert, **nicht** der geheime `MCP-Session-Id`-Header.

Migration 12 ergänzt `mcp_servers`, `mcp_server_requests` und `mcp_attempts`.
Ereignisschema 7 und Auftragsexport 9 enthalten die neuen optionalen Bindungen und
Versuche. Leere optionale Felder verändern die Serialisierung historischer
Laufkonfigurationen nicht. Lesen und Export prüfen Identität, Referenzen, Hashes,
Revision und bestätigte Ergebnisschemata; beschädigte Daten werden nicht repariert.

## Unklarer Ausgang und Wiederaufnahme

`dispatch_committed` bedeutet ausschließlich „dauerhaft zugelassen“. Es beweist
weder Ausführung noch Nichtausführung. Werkzeugfehler, Timeout, Verbindungsabbruch,
Abbruchanforderung oder ein verlorener Ergebnisabschluss behalten diese Unsicherheit.
Der Runtime-Einstieg speichert nach Möglichkeit einen geschützten Antwortbeleg und
unterbricht den Lauf. Bei Prozess- oder Datenbankausfall bleibt der ursprüngliche
Marker erhalten; die bestehende Lease-Wiederherstellung unterbricht den Lauf.

Eine unterbrochene unsichere Aktion kann weder automatisch fortgesetzt noch als
wirkungslos abgebrochen werden. Der Schreibbesitz bleibt erhalten. Der private
Abgleich verlangt exakte Versuchsversion, Aufrufhash und einen Belegverweis.
Er erfasst entweder ein geprüftes Ergebnis oder die nachgewiesene Nichtausführung,
verbraucht den offenen Schritt und lässt den Lauf weiterhin unterbrochen.
Erst eine gesonderte Wiederaufnahme setzt ihn fort. Der alte Werkzeugaufruf wird
auch bei einer neuen HTTP-Sitzung nicht erneut ausgeführt.

Ein Beleghash authentifiziert keinen Prüfer und beweist keine Beendigung des
entfernten Servers. Vor Produktfreigabe benötigt der Abgleich Berechtigungsprüfung,
gesicherte Belege und eine verständliche Oberfläche. Ein neues ausdrücklich
autorisiertes Werkzeugkommando muss eine neue Aufruf-ID erhalten.

## Grenzen der Garantie

- HTTP und lokale Datenbank bilden keine verteilte atomare Transaktion. Die
  Implementierung verhindert automatische Wiederholung, verspricht aber keine
  Exactly-once-Ausführung auf einem beliebigen entfernten Server.
- Eine HTTP-Verbindung zu schließen bestätigt nicht das Ende des entfernten
  Programms. Abbruch während einer möglichen Änderung führt deshalb zu Unsicherheit.
- Entzug nach der letzten lokalen Prüfung kann eine bereits gesendete Aktion nicht
  zurückholen. Entfernte Nebenwirkungen müssen vor dem Abgleich separat geklärt sein.
- Der ursprüngliche Dispatcher erhält eine ausdrücklich verbundene Sitzung. Der
  ergänzte persistente Dispatcher öffnet den aktuellen verschlüsselten Checkpoint.
  Beide verbinden nach unklarem Ausgang nicht automatisch neu. Auswahl und
  Modelllauf sind inzwischen angeschlossen; vollständige Einrichtung,
  Schlüsselwiederherstellung und Server-KMS fehlen. Der ergänzte
  Dienst löst verschlüsselte Credentials und native Schlüssel bereits auf.
- OAuth, stdio-Executor, Session-DELETE, GET-Resume, Remote-Cancel und Tests mit
  echten MCP-Servern stehen weiterhin aus; siehe [Transport](MCP-TRANSPORT.md).
- Das lokale Register ist keine Enterprise-Mandantentrennung und keine vollständige
  Netzwerk-Zielrichtlinie. M4 und M6 bleiben offen.

## Prüfung

Die Rust-Suites verwenden eigene HTTP-Testserver und temporäre Datenbanken. Eine
synthetische Aktion schreibt und synchronisiert tatsächlich eine Datei. Geprüft
werden vier Modi, veränderte Argumente nach Freigabe, konkurrierende Zulassungen und
Abschlüsse, alte Leases, falsche Endpoints, Budgetgrenzen, Entzug, Konfigurations-
konflikte, Datenbankfehler, beschädigte Belege und Exportintegrität.

Der harte Absturztest beendet einen echten Worker-Kindprozess nach der bestätigten
Dateiwirkung des Testservers, während die HTTP-Antwort noch aussteht. Die Datenbank
wird erneut geöffnet, der Lauf unterbrochen, das Ergebnis abgeglichen und separat
fortgesetzt. Auch eine neue Verbindung bewirkt keinen zweiten Schreibvorgang.
Die kontrollierte Testaktion hat nach dem Append keine weitere Arbeit; dieser Test
beweist keine Beendigung beliebiger entfernter MCP-Programme.

```powershell
cargo test --locked -p cowork-storage -p cowork-runtime --test mcp
python tools/qualification/mcp.py --workspace
# COWORK_TEST_POSTGRES muss eine eigene leere Testdatenbank bezeichnen:
cargo test --locked -p cowork-storage --test mcp postgres_mcp_contract -- --ignored
```

Aktuelle Plattformnachweise und offene Abnahmen stehen in [VERIFY.md](VERIFY.md).
