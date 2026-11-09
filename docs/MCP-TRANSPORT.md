# MCP-Transport — privater M4-Baustein

Quellstand 12. Oktober 2026: Die neutrale Transportbibliothek enthält inzwischen
auch den echten Legacy-SSE-Adapter (`crates/mcp/src/legacy_sse.rs`). Er hält den
originalen GET-Stream und bindet die POST-Adresse an denselben Ursprung. Der
Android-Privatpfad wird auf dieser Bibliothek aufgebaut; seine vollständige
isolierte stdio-/Werkzeugausführung bleibt in Umsetzung. OAuth und asynchrone
Pflicht-Tasks waren in diesem historischen Stand nicht unterstützt. Der aktuelle
additive MCP63-Schnitt (13. Oktober) handelt echte 2025-11-25 Taskfähigkeiten aus,
nimmt Task-IDs ausschließlich aus Original-ACKs an und erfasst auch terminale
`tasks/result`-Fehler. Tatsächliche Produktpfade liefern eigene verschlüsselte
Journal-/Actor-/Modellautorität und taskgebundene menschliche Handler. Der
Form-only-Konstruktor kündigt Sampling ohne gehaltenen originalen Modellkontext
nicht an; weder URL-Elicitation noch Remote-Modell-/Toolwahl wird angekündigt.
POST-SSE, der an derselben Originaloperation gehaltene GET-SSE-Stream und
originale stdio-Pipes verarbeiten diese gebundenen Rückfragen. GET und POST
verwenden dieselbe ausgehandelte Sitzung/Version und eine einzelne Original-RPC-
Zulassung; weder Wiederverbindung noch Werkzeug-POST-Replay wird eingeführt.
OAuth-Produktpfade lösen vor Task-RPCs und nach langen menschlichen Wartezeiten
vor einem Antwort-POST den aktuellen Zugang derselben eingefrorenen Konfiguration
auf. Sie ändern ausschließlich den Bearer, prüfen ihre originale Actor-/Rollen-
Autorität erneut und behalten URL, Protokoll, Sitzung und Epoch unverändert.
Der aktuelle Windows-Workspace- und Desktopcompile erfasst die gebundenen
Native-/Team-/privaten Geräte- und signierten Offlinepfade. Ein enger tatsächlicher
HTTP-GET-SSE-Austausch mit nach der Eingabe erneuertem Antwortbearer besteht.
Originalquittungs-GETs können lokale Freigaben nach GUI-Reload lesen und führen
keinen Task-/Werkzeug-/Modell- oder Antwort-POST aus. Aktuelle Pakete und echte
Anbieter-/Betriebsabnahme bleiben separat. Die folgenden
Aussagen und Prüfberichte vom 8. Oktober behalten ihren historischen Quellenumfang.

Stand: 8. Oktober 2026. `cowork-mcp` implementiert einen begrenzten HTTP-Client.
Die Bibliothek ist jetzt mit einem privaten dauerhaften Journal, der gemeinsamen
Policy und einem Runtime-Einstieg verbunden; siehe [MCP-JOURNAL.md](MCP-JOURNAL.md).
Bestätigte Sitzungen können über einen verschlüsselten Checkpoint fortgesetzt werden;
siehe [MCP-SESSIONS.md](MCP-SESSIONS.md).
Modellschleife, revisionierter Katalogimport und Werkzeugauswahl sind angeschlossen;
siehe [MCP-PRODUCT.md](MCP-PRODUCT.md).
Dies ist kein Nachweis vollständiger MCP- oder M4-Funktionsabdeckung.

## Implementierter Ablauf

`Endpoint::new` übernimmt eine ausdrücklich gewählte URL und optional einen bereits
beschafften Bearer-Schlüssel. Die Bibliothek liest keine Umgebungsvariablen oder
persönlichen Konfigurationsdateien und startet keinen lokalen MCP-Prozess.
HTTPS ist erlaubt; unverschlüsseltes HTTP verlangt eine Loopback-IP als Literal.
URL-Benutzerinformationen, Query und Fragment sind ausgeschlossen. Redirects,
Umgebungs-Proxys, HTTP-Wiederholungen und wiederverwendete Verbindungen sind deaktiviert.
HTTP/1.1 ist die gegenwärtig qualifizierte Transportvariante.

Die Verbindung beginnt mit `initialize`, ausschließlich Version `2025-11-25`,
gefolgt von `notifications/initialized`. Die Antwort muss die Tools-Fähigkeit
deklarieren. Eine erhaltene Sitzungs-ID und die Protokollversion werden bei
Folgeanfragen mitgesendet. Der Client meldet keine Sampling-, Roots-, Elicitation-
oder Task-Fähigkeiten. Dies verwendet den offiziellen
[MCP-Lebenszyklus](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

`discover` liest alle Katalogseiten bis zum gesetzten Limit. Doppelte Werkzeugnamen,
wiederkehrende Cursor, ungültige Definitionen und nicht unterstützte Pflicht-Tasks
brechen die Entdeckung ab. Der Kataloghash umfasst die vollständigen Definitionen,
sortiert nach Werkzeugnamen. Beschreibungen und Annotationen bleiben untrusted
Daten. Insbesondere werden `readOnlyHint` und `idempotentHint` nicht in Rechte oder
eine Wiederholungserlaubnis übersetzt. Die externe Schnittstelle folgt dem
[MCP-Werkzeugvertrag](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).

`prepare` verlangt den erwarteten Kataloghash und validiert die Argumente. Das
Ergebnis enthält Werkzeugname, Argumente und einen Hash über Endpoint, Protokoll,
Katalog, Aufruf, lokale Verbindungskennung und die bereits erzeugte JSON-RPC-ID.
Es ist weder klonbar noch deserialisierbar und zusätzlich an
eine konkrete Sitzung und deren Verbrauchszähler gebunden. Ein erfolgreicher
Aufruf macht andere zuvor vorbereitete Aufrufe derselben Sitzung ungültig.
Damit muss jede folgende Aktion neu vorbereitet werden.

`call` verwendet JSON-RPC über POST. Sowohl JSON-Antworten als auch SSE mit
Servernachrichten und leeren Eröffnungsereignissen werden verarbeitet.
Ein Ping wird beantwortet; andere
Serveranfragen erhalten einen Methodenfehler und lösen keine lokale Aktion aus.
Eine Katalogänderungsnachricht invalidiert die Auswahl für den nächsten Aufruf.
Resource-Links und eingebettete Ressourcen werden als Inhalt zurückgegeben;
es findet kein zusätzlicher Datei- oder Netzwerkzugriff statt.
Die Transportgrundlage ist
[Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

## Ausgang und Wiederholung

Vor dem ersten Netzwerk-Await einer Werkzeugausführung wird die Sitzung gesperrt.
Nur eine passende, vollständig geprüfte Erfolgsantwort macht sie wieder nutzbar.
Timeout, Verbindungsabbruch, ungültige Antwort, JSON-RPC-Fehler, `isError: true`,
Sitzungsablauf oder eine verletzte Ausgabespezifikation lassen sie gesperrt.
Auch das Verwerfen des laufenden Rust-Futures führt nicht zu einer nutzbaren
Sitzung. Es gibt keine automatische Wiederverbindung und keinen POST-Replay.
Ein HTTP-404 erfordert eine gesonderte, explizite neue Verbindung; der alte
Werkzeugaufruf bleibt ungeklärt.

`CallFailure.outcome_uncertain` bedeutet, dass für diesen Dispatch keine sichere
Aussage über die Wirkung vorliegt. `false` beschreibt ausschließlich eine vor
Netzwerkzugriff abgewiesene neue Dispatch-Gelegenheit. Es ist **kein Nachweis**,
dass ein früherer Aufruf nichts verändert hat, und darf keinen früheren
dauerhaften Unsicherheitsmarker löschen. Eine neue `Session` verhindert für sich
allein ebenfalls keine erneute Wirkung auf dem Server.

JSON-RPC- und Werkzeugfehler können geschützte Antwortbelege enthalten. Die
Fehleranzeige und `Debug` geben deren Inhalt nicht aus. Schlüssel und Sitzungs-IDs
sind sensible HTTP-Header; Endpoint, Sitzung und vorbereitete Aufrufe haben keine
abgeleitete `Debug`-/Serialisierungsimplementierung. Diese Maßnahmen ersetzen
nicht die noch ausstehende Credential-Verwaltung. Geschützte Antwortbelege können
im dauerhaften Journal abgelegt werden; authentifizierte menschliche Prüfung und
deren Produktoberfläche fehlen noch.

## Grenzen

| Grenze | Standard |
| --- | --- |
| HTTP-Aufruf / gesamte Katalogentdeckung | 30 Sekunden |
| Empfangene Bytes je HTTP-Anfrage einschließlich SSE | 1 MiB |
| JSON-RPC-Nachrichten je Anfrage | 128 |
| Katalogseiten / Werkzeuge | 16 / 128 |
| Gesamter Katalog | 2 MiB |
| Einzelnes Schema | 16 KiB, 24 Ebenen, 2.048 Knoten |
| Argumente / strukturierte Ausgabe | 256 KiB, 24 Ebenen, 8.192 Knoten |
| Inhaltsblöcke je Ergebnis | 256 |

Konfigurationen haben zusätzliche feste Obergrenzen. Der Deadline-Zähler wird
durch Fortschrittsnachrichten nicht verlängert. Bytes werden auch ohne
`Content-Length` gezählt; SSE wird inkrementell verarbeitet. Doppelte JSON-
Schlüssel, unvollständige Nachrichten und falsche JSON-RPC-IDs werden abgewiesen.

Die gepinnte Bibliothek `jsonschema 0.58.6` läuft ohne HTTP-/Datei-Resolver und
explizit im Offline-Modus. Lokale JSON-Pointer-Referenzen sind möglich, externe,
zyklische und dynamische Referenzen nicht. Referenzexpansion und Verknüpfungen
sind begrenzt. Reguläre Ausdrücke verwenden den Rust-Regex-Modus; Backreferences
und Lookaround werden als nicht unterstütztes Schema zurückgewiesen. Das ist
ein bewusst eingeschränktes Profil und kein Versprechen beliebiger JSON-Schema-
Kompatibilität. Die synchronen, größenbegrenzten Schemaoperationen sind keine
eigenständige Betriebssystem-CPU-/Speicherisolation.

## Noch erforderliche Produktintegration

1. Das revisionierte Register besitzt nun Owner-Verwaltungsrouten und einen
   Katalogimport. Automatische Entdeckung und Credential-Einrichtung fehlen;
   native Credential-Auflösung ist implementiert, siehe MCP-SECRETS.md.
2. Eingefrorene Bindungen, gemeinsame Policy, Auswahl und Sitzungsdienst sind an
   die Modellschleife angeschlossen. Vollständige Credential-Verwaltung und
   Schlüsselwiederherstellung bleiben offen.
3. Das dauerhafte Journal erteilt auch im normalen Modelllauf bei Wiederholung
   keine Sendeberechtigung. Reale MCP-Produktabnahmen bleiben erforderlich.
4. Antwortbelege und Abgleich sind privat implementiert. Authentifizierte
   menschliche Entscheidungen benötigen Belegverwaltung und Produktoberfläche.
5. Verständliche Einrichtung, Katalogvorschau, Freigaben, Diagnosen und Abgleich
   in DE/EN sowie realer End-to-End-Test auf Desktop und Web.
6. Qualifizierte stdio-Executor-Anbindung, OAuth, gezielte Sitzungsbeendigung,
   Resume über GET ohne Werkzeug-POST-Replay sowie erforderliche weitere
   Serverfähigkeiten. Legacy-SSE und asynchrone MCP-Tasks sind nicht implementiert.
7. Kompatibilitätsprüfungen mit ausgewählten echten MCP-Servern sowie Mandanten-,
   Last- und Ausfallprüfungen. Die lokalen Protokoll-Fixtures ersetzen diese nicht.

## Reproduzierbare Prüfung

```powershell
cargo test --locked -p cowork-mcp
cargo clippy --locked -p cowork-mcp --all-targets -- -D warnings
python tools/qualification/mcp.py --workspace
```

Der Prüfer schreibt Quellenhashes und Ergebnisse unter `.local/mcp-<ID>`.
Er verwendet bereits verfügbare Cargo-Abhängigkeiten offline. Die Linux-
Workspace-Prüfung läuft zusätzlich als UID 10001 in einem Container ohne
Host-Mounts und ohne externes Netzwerk. Lokale HTTP-Fixtures beobachten echte
Anfragen und zählen ihre synthetischen Änderungen; sie prüfen unter anderem
verlorene Antworten nach einer Wirkung. Es werden keine persönlichen MCP-Server
oder Live-Daten verwendet. Aktuelle Nachweise stehen in `VERIFY.md`.
