# Dauerhafte native Schlüsseleinrichtung

**Quellstand 12. Oktober 2026:** Die Oberfläche bietet inzwischen verschlüsselte
Schlüsselsicherung, Wiederherstellung der ursprünglichen Schlüssel, ausdrückliche
Umschlüsselung und kontrollierte Bereinigung nicht mehr benötigter Schlüssel.
Aufgegebene Setupvorgänge ohne Bestätigung werden über ihre ursprüngliche
Vorgangskennung geklärt. Eine solche Bereinigung aktiviert den Kandidaten nicht
und verändert weder den ursprünglichen Setupbeleg noch vorhandene Quittungen.

Die Einrichtung hält eine SQL-Schreibsperre während der tatsächlichen nativen
Erzeugung. Auch nach Timeout oder Abbruch des Aufrufers kann die Bereinigung
keine noch laufende Erzeugung überholen. Vor der OS-Löschung wird ein dauerhafter
Entzugsbeleg gespeichert; aktive Auswahl und sämtliche MCP-Ciphertextverweise
blockieren die Löschung. Fehlendes Schlüsselmaterial kann nur für einen tatsächlich
aufgegebenen Originalvorgang bestätigt werden. Ein verlorener Reply verwendet
dieselbe ursprüngliche Quittung und erteilt keine neue Einrichtungsberechtigung.

Die kleine SQLite-Prüfung dieses Bereinigungswegs besteht. Tatsächliche native
Windows-/Linux-Recovery, Dialoge und aktuelle Installer sind gesondert abzunehmen.
Team-/Modellproviderzugänge verwenden ihren eigenen serverseitigen Schlüsselspeicher;
dieser Weg ersetzt keine KMS- oder Team-Masterschlüsselrotation. Die nachfolgende
Beschreibung und ihre Nachweise dokumentieren den historischen Einrichtungsschnitt.

Stand: 8. Oktober 2026. Der lokale Rust-Dienst kann die aktive Schlüsselwahl
einrichten, rotieren, nach einem unklaren Abschluss abgleichen oder einen offenen
Vorgang aufgeben. Migration 15 speichert die Instanzkennung, Revision, aktive
Schlüsselbestätigung und unveränderliche Vorgangsidentität. Der eigentliche Schlüssel
bleibt ausschließlich im nativen Schlüsselspeicher; siehe [MCP-SECRETS.md](MCP-SECRETS.md).

Die lokale API, Tauri-Befehle und die gemeinsame Oberfläche unter **Lokale Sicherheit**
sind implementiert. Änderungen verlangen einen nativen Bestätigungsdialog. Dessen
interaktive Betriebssystemabnahme steht noch aus. Der Dienst ist bei PostgreSQL-Betrieb
gesperrt; der gemeinsame PostgreSQL-Speichervertrag ist keine Server-KMS-Anbindung.

## Ablauf und Wiederholung

1. Status lesen. Ein leerer Speicher hat Revision 0. Lesen und Daemon-Start legen
   weder einen Datenbankdatensatz noch einen Schlüssel an.
2. Einrichtung ausdrücklich mit Idempotenzschlüssel und erwarteter Revision
   anfordern. Eine Datenbanktransaktion reserviert eine neue zufällige Schlüsselkennung
   und speichert einen offenen Vorgang. Die Revision steigt um eins.
3. Nur der Aufrufer, der diese Reservierung erstmals bestätigt erhält, bekommt eine
   einmalige Erzeugungsmöglichkeit. Er erzeugt den nativen Schlüssel und liest ihn
   zurück. Ein vorhandener Eintrag wird niemals ersetzt oder automatisch übernommen.
4. Eine zweite Datenbanktransaktion bestätigt den Schlüssel und die aktive Auswahl
   gemeinsam. Die Revision steigt erneut. Erst dann ist die Auswahl eingerichtet.

Datenbank und Betriebssystem bilden keine gemeinsame Transaktion. Ein verlorener
Abschluss kann deshalb einen offenen Vorgang mit bereits vorhandenem Schlüssel
hinterlassen. Dieselbe Startanfrage gibt immer den gespeicherten Vorgang zurück,
ohne erneut einen Schlüssel zu erzeugen. Ein offener Vorgang sperrt neue Einrichtungen.

Der explizite Abgleich liest ausschließlich den zuvor reservierten Schlüssel und
bestätigt ihn, wenn Vorgang und Revision weiterhin passen. Er erzeugt keinen Ersatz.
Fehlt der Schlüssel oder ist der native Speicher gesperrt, bleibt der Vorgang offen.
Ein bewusst aufgegebener Vorgang wird endgültig geschlossen. Aufgeben löscht keinen
OS-Eintrag: Ein zuvor blockierter nativer Aufruf könnte noch nachträglich fertig werden.
Sein verspätetes Ergebnis kann den aufgegebenen Vorgang nicht mehr aktivieren.

Eine neue Rotation erfordert eine neue Anfrage mit der aktuellen Revision. Die alte
aktive Auswahl bleibt bis zum bestätigten Abschluss nutzbar. Wiederholung eines
bereits abgeschlossenen alten Vorgangs verändert die inzwischen aktive Auswahl nicht.
Bestätigungsbelege enthalten deshalb getrennt den Zustand des betreffenden Vorgangs
und den aktuellen Zustand des Speichers.

## Identität und Schutzzonen

Die Datenbank enthält eine HMAC-SHA-256-Bestätigung des zufälligen 256-Bit-Schlüssels,
gebunden an Format, Zweck, Instanz- und Schlüsselkennung. Beim Laden der aktiven
Auswahl wird genau das geladene Schlüsselmaterial geprüft und weiterverwendet. Ein
ausgetauschter Schlüssel unter derselben Kennung wird abgewiesen. Die Bestätigung ist
kein Geheimnis und wird trotzdem nicht über die öffentliche Status-API ausgegeben.

Öffentliche Antworten enthalten nur Revision, eingerichtet/verfügbar, offene
Vorgangskennung und Vorgangszustand. Anfragen enthalten keine Schlüssel, Token oder
vom Client vorgegebenen nativen Schlüsselkennungen. Unbekannte JSON-Felder werden
abgewiesen. Ein gesperrter oder fehlender Speicher ergibt weiterhin
`configured: true`, aber `key_available: false`; er wird nicht still ersetzt.

Native Aufrufe laufen auf begrenzten Hilfsthreads. Einrichtung und Status warten
höchstens zehn Sekunden einschließlich Slotvergabe. Ein noch laufender OS-Aufruf
behält seinen Slot bis zum tatsächlichen Ende. Timeout gilt bei einer Erzeugung als
unklarer Ausgang, nicht als Nachweis, dass nichts geschrieben wurde.

## Native API

Alle Pfade liegen unter `/api/v2/native`. Sie verlangen die Daemon-Anmeldung und
zusätzlich `x-cowork-native`. Jede Anfrage mit einem `Origin`-Header wird vor dem
Lesen des Bodys abgewiesen. Die native Berechtigung darf nicht in Renderer-, Browser-
oder Agentenzustand gelangen. Die Tauri-Befehle bestätigen die Einrichtungsabsicht
im nativen Vertrauensbereich.

| Methode und Pfad | Wirkung |
| --- | --- |
| `GET /secrets` | Status und Verfügbarkeit lesen |
| `POST /secrets` | Einrichtung/Rotation mit `idempotency_key` und `base_revision` beginnen |
| `GET /secrets/requests?key=…` | Gespeicherten Vorgang nach unbestätigter Antwort suchen |
| `POST /secrets/reconcile` | Reservierten Schlüssel mit `operation_id` und Reservierungsrevision abgleichen |
| `POST /secrets/abandon` | Offenen Vorgang mit derselben Bindung endgültig aufgeben |

Konflikte werden nicht automatisch wiederholt. `native_key_unavailable` bezeichnet
fehlenden, veränderten oder nicht erreichbaren nativen Schlüsselzugriff.
`uncertain_effect` bei Einrichtung verlangt das Lesen des vorhandenen Vorgangs und
eine gezielte Entscheidung. Ein Datenbankfehler nach OS-Erzeugung wird ebenfalls
durch Beobachten/Abgleichen des ursprünglichen Vorgangs behandelt.

## MCP-Nutzung und Grenzen

### Bedienung im Desktop

Der Navigationseintrag **Lokale Sicherheit** öffnet den Schlüsselspeicher. Der Bereich
zeigt getrennt an, ob eine Auswahl eingerichtet, aktuell verfügbar oder noch offen ist.
Statusabfragen erzeugen keine Schlüssel. Es gibt keine wiederkehrende Statusabfrage und
keine automatische Wiederholung einer Änderung. Ein offener OS-Dialog wird für diesen
Einrichtungsbereich serialisiert. Die Texte in WebView und nativen Dialogen stammen aus
derselben DE-/EN-Datei `packages/ui/src/secretMessages.json`.

Die fünf Tauri-Befehle akzeptieren nur die vorhandenen Status-/Anfragetypen und die
Sprache. Hostadresse, HTTP-Berechtigungen und Dialogtexte kommen aus dem nativen Code.
Die Brücke akzeptiert nur das eigene Hauptfenster mit erlaubter Anwendungsadresse.
Sie verwendet dieselbe begrenzte, redirect- und proxyfreie HTTP-Brücke wie die Ordnerverwaltung.

Bei verlorener Antwort bleibt die Anfrage in der geöffneten Ansicht erhalten. **Unbestätigte
Anfrage prüfen** liest ihren gespeicherten Beleg, ohne eine Einrichtung zu wiederholen.
Ein bereits gespeicherter offener Vorgang bleibt nach Schließen und Wiederöffnen des
Fensters über den Daemon-Status zugänglich. Abgleichen und Aufgeben beziehen sich auf
die angezeigte Vorgangskennung und Revision und werden nochmals nativ bestätigt.

Ein neuer Schlüssel ist keine Wiederherstellung verlorener Schlüssel. Die Oberfläche
und der Rotationsdialog erklären, dass alte Daten ihre ursprünglichen Schlüssel
weiterhin benötigen. Der Browser zeigt einen Desktop-Hinweis und besitzt keine
nativen Änderungsaktionen. Interne Kennungen sind in den Diagnosen zusammengefasst.

### Historische Grenzen des Einrichtungsschnitts vom 8. Oktober

Der private Runtime-Einstieg `dispatch_configured_mcp_tool` liest die gespeicherte
aktive Auswahl auch nach einem Neustart. Neue Sitzungen verwenden diese Auswahl;
bestehende verschlüsselte Sitzungen verlangen weiterhin ihren ursprünglichen
Schlüssel. Rotation führt keine erneute MCP-Initialisierung aus. Ein fehlender alter
Schlüssel sperrt die betroffene Sitzung und bewirkt keinen Rückfall auf den neuen.

Es gibt keine automatische Umschlüsselung bestehender Daten, keine Schlüsselbereinigung,
keine verschlüsselte Schlüsselsicherung und keinen vollständigen Wiederherstellungsweg.
Offene und aufgegebene Vorgangskennungen bleiben zur späteren kontrollierten Klärung
erhalten. Ein authentischer Datenbank-Rollback und ein kompromittierter OS-Benutzer
werden durch diese Bestätigungen nicht verhindert. Modell-Provider-Credentials sind
noch nicht auf diesen Dienst umgestellt. Die MCP-Modellschleife, Konfigurations-API
und Katalogimport/Auswahl sind inzwischen angeschlossen; vollständige Einrichtung
und belegte Klärung bleiben offen. Siehe [MCP-PRODUCT.md](MCP-PRODUCT.md).

## Nachweise

Die regulären Tests prüfen konkurrierende Reservierung/Bestätigung, alte Wiederholungen,
Revisionen, fehlgeschlagene Datenbankabschlüsse, beschädigte Metadaten, nachträgliche
OS-Abschlüsse nach Aufgeben, veränderte Schlüssel und die Fortsetzung einer bestehenden
MCP-Sitzung nach Rotation. HTTP-Tests prüfen beide Berechtigungen und Origin-Abweisung
vor jedem Body-Zugriff. Derselbe Speichervertrag läuft separat gegen PostgreSQL.

Der zusätzliche reale Prozessprüfer startet einen eigenen Kindprozess mit frischer
SQLite-Datenbank und ausschließlich synthetischen Schlüsselkennungen. Er beendet ihn
hart nach bestätigter Einrichtung sowie nach OS-Erzeugung mit absichtlich gescheitertem
Datenbankabschluss. Danach prüft er Wiederaufnahme, Abgleich, fehlende Ersetzung und
gezielte Löschung der eigenen Testschlüssel. Linux verwendet einen privaten DBus und
ein temporäres GNOME-Keyring-Profil. Aktuelle Ergebnisse und ihre Quellenmanifeste
stehen in [VERIFY.md](VERIFY.md). Das ersetzt keine interaktive Desktop- oder KMS-Abnahme.
