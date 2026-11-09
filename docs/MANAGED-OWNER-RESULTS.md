# Lokale Runtime-Ergebnisse im Owner-Pfad

Stand: 31.10.2026. Der aktuelle Quellstand enthält den öffentlichen Owner-Anschluss
für getrennte lokale Ergebnisversionen. Der bestehende Schema 115-Empfänger und
seine strikten Empfangsrechte bleiben die Grundlage.

Die Ansicht „Runtime-Ergebnisse“ erscheint bei bestätigten Arbeitskopien eines
verwalteten Einzelauftrags. Sie zeigt noch nicht lokal gespeicherte, unvollständige
und vollständig geprüfte Ergebnisse getrennt. Nur ein ausdrücklicher Abrufknopf
beginnt oder setzt eine Übertragung fort. Nach einer verlorenen Antwort wird der
lokale Stand gelesen; es erfolgt kein automatisches zweites Empfangs-POST.

Ready-Manifeste und Downloads lesen ausschließlich die bereits erworbene lokale
Version. Sie benötigen keinen konfigurierten oder erreichbaren Runner. Aktuelle
Owner-, Auftrags-, Lauf-, Controller-, Quell-, Arbeitskopie- und Abbruchprüfungen
sowie die vollständige native Inhaltsprüfung bleiben erforderlich. Beschädigte
oder fehlende Inhalte werden nicht durch einen entfernten Abruf repariert.

## API

Alle Routen liegen unter `/api/v2/orders/{order}/runs/{run}`, verwenden die
bestehende Owner-Anmeldung und liefern `Cache-Control: no-store`.

| Methode | Pfad | Antwort |
|---|---|---|
| GET | `/managed-results` | `OwnerManagedResultList` |
| POST | `/managed-results/{attempt}/receive` | `OwnerManagedResultEntry` |
| GET | `/managed-result-versions/{version}/manifest` | `OwnerManagedResultManifest` |
| GET | `/managed-result-versions/{version}/chunk` | Binärblock mit Längen- und Hashheadern |

Chunk-Abfragen benötigen `path`, `offset` und `expected_sha256`. Versions-IDs und
Hashes ersetzen keine Zugriffsrechte. Die vollständige Liste ist auf 128 originale
Prozessversuche begrenzt; größere Listen liefern einen ausdrücklichen Fehler.
`version: null` bedeutet, dass keine lokale Empfangsabsicht besteht. Es verspricht
keine entfernte Verfügbarkeit. Native Custody, Grants und signierte interne
Protokollobjekte werden nicht als öffentliche DTOs ausgegeben.

Das SDK prüft Zuordnung, Manifest, portable Pfade, Größen, Blocklayout und alle
Hashes. Sein Abbruchsignal gilt auch beim erneuten Manifestabruf und vor der
Dateiübergabe. Nach bereits ausgelöstem nativen Speichern bleibt dessen originale
Quittung maßgeblich; ein Ansichtswechsel erzeugt keine erfundene Abbruchquittung.

## Nachweise und offene Abnahme

Im integrierten Quellstand bestehen 628 Frontendtests in 118 Dateien, 33 gezielte
Owner-/Download-/Arbeitskopietests, Typprüfung, Web-Build und der Abgleich aller 735
generierten Verträge. Die 441 erfassten Frontendquellen sind unverändert archiviert.
Der frische integrierte Windows-Runtime-Prüfstand besteht 55 SQLite- und 55 isolierte
PostgreSQL-Fälle mit demselben Testprogramm. Alle neuen Datenbanken sind entfernt.
Auch beide Owner-Routentests bestehen im aktuellen Main. Die 698 Runtime- und
834 Daemonquellen sind unverändert archiviert. Ältere 40/40- und 46/46-Nachweise
bleiben an ihre eigenen Quellen gebunden.

Der vierte tatsächliche Controllerlauf hat eine Kollision zwischen nativer
Speicherreservierung und paralleler Umgebungsbeobachtung nachgewiesen. Ein enger
Pool-Synchronisationsfix besteht den getrennten Red-/Green-Komponententest. Die
vollständige Controllergegenprobe besteht inzwischen einmal mit echtem Gast,
Originalquittung, lokalem Ready-Ergebnis und exaktem Replay. Ein vorheriger HTTP 409
beim Missing-Abruf bleibt ungeklärt; der einzelne erfolgreiche Lauf ist keine
Stabilitätsabnahme. Ebenso offen bleiben die aktuelle Windows-App-Abnahme,
ausdrücklich bestätigte Weiterverwendung der Runtime-Ausgaben und die
Produktfreigabe ihrer belegten Kapazität.

Nachweise: `.local/owner-managed-results-ui-20261031/main-integration/`
und `MANAGED-RUNTIME-CONTROLLER-VALIDATION.json`. Keine Installation oder
Veröffentlichung ist Teil dieses Prüfstands.

Am 01.11.2026 ist außerdem der [frische Ergebnislesekontext nach einem Stopp](MANAGED-RESULT-CONTEXT.md)
integriert. Die tatsächliche Probe verbindet Gaststart, Store.cancel, ursprüngliche
signierte Quittung und ausdrücklichen lokalen Ready-Abruf. Der Auftrag bleibt nach
reinem Abschlussabgleich unterbrochen. Originale, bestätigte Eingabekopie und der
ursprüngliche Prozessversuch bleiben erhalten. Der eigene Main-Prüfstand und die
getrennten physischen/PostgreSQL-Belege stehen im
neuen Prüfbeleg (development artifact omitted from public sources). Die oben genannten
historischen Fallzahlen bleiben an ihren damaligen Stand gebunden.
