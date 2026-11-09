# Verpflichtendes Originaljournal und lesender Wiederanlauf

Die gemeinsame native `PublicationAttempt` besitzt jetzt zwingend ein eigenes
`OriginalJournal`. Der bestehende CLI-Publish und die öffentlichen nativen
Methoden verwenden denselben Besitzer. Es gibt keinen optionalen zweiten
Veröffentlichungspfad und keinen Aufruf mit einem booleschen „Intent bestätigt“.
Journalzustände oder lokale Identitätsnummern aus JSON können dieses Objekt
nicht erzeugen.

Nach vollständiger Vorbereitung von signiertem Input und neun Rollen entsteht
neben der ursprünglichen Versuchswurzel `<Versuch>-journal`. Ein bestehender
Eintrag wird nicht ersetzt. Der Journalname unterliegt derselben Grenze von
96 ASCII-Zeichen; der Versuch muss Platz für das Suffix lassen. Fehlgeschlagene
Vorbereitungen bleiben begrenzte unqualifizierte Arbeitsstände.

Das private Verzeichnis enthält ausschließlich `owner.lock`, `state.a` und
`state.b`. Der ursprüngliche Besitzer hält die exklusive native Sperre sowie
alle ursprünglichen Verzeichnis- und Dateihandles. Beide Zustandsdateien werden
vor Verwendung auf je 256 KiB physisch reserviert. Die 512 KiB Journalpayload
kommen zusätzlich zu den Paket-/Rollenbytes; Verzeichnis-, Inode- und
Blockrundungsbedarf bleibt Aufgabe des unabhängig begrenzten Management-
Dateisystems. Der Payloadwert erzeugt keine Volumequote.

Jeder Frame besitzt feste Größe, Sequenz, Länge, Prüfsumme und Nullpadding.
Das Journal bindet das tatsächlich gehaltene signierte Manifest samt genauer
Signatur, Manifesthash und signierter Release-Sequenz. Es bindet außerdem die
ursprüngliche lokale Quellwurzel und das vorab gewählte Ziel. Die Signatur wird
gegen die unabhängig konfigurierte `TrustRoot` geprüft. Private Signierschlüssel,
Transporttoken oder Projektdateien stehen nicht im Journal. Native Kennungen
bleiben lokale Prüfmerkmale und werden nicht in der öffentlichen Statusquittung
als portable Fähigkeiten exportiert.

Die Reihenfolge ist verbindlich:

1. `prepare` übernimmt und prüft alle Paketrollen und erzeugt den ursprünglichen
   Journalbesitzer mit dauerhaftem Zustand `prepared`.
2. `arm_publication` bindet die tatsächlich gehaltene Zielcapability und schreibt
   den vollständigen `intent`. Originalprüfung, `fsync` und exakter Readback
   müssen vor jedem Rename bestanden sein.
3. `dispatch_publication` verlangt denselben ursprünglichen Besitzer und genau
   diesen bestätigten Intent. Erst danach wird der einmalige Kernel-Rename
   mit `RENAME_NOREPLACE` ausgelöst. `begin_publication` und `publish` verwenden
   ebenfalls diese beiden Schritte.
4. `observe_publication` prüft den Originalbaum, synchronisiert beide Eltern und
   bestätigt das tatsächliche Ergebnis im ursprünglichen Journal. Erst die
   bestätigte Abschlussphase liefert eine vollständige Publish-Quittung.

Ein Intent-I/O-Fehler sperrt den Dispatch dieses Besitzers. Die Sperre wird
nicht automatisch zurückgesetzt. Ein Fehler nach möglichem Rename lässt den
Ausgang ungeklärt; derselbe ursprüngliche Besitzer kann ausschließlich die
ursprüngliche Wirkung beobachten und seinen identischen Abschlussframe erneut
bestätigen. Er führt keinen weiteren Rename aus. Eine fehlende Datei oder ein
ersetztes Original wird nicht durch einen gleichen Pfad oder gleiche Bytes
adoptiert.

## Nach Ende des ursprünglichen Prozesses

`read_publication_history` liefert ausschließlich Daten. Es erfordert das
vorhandene private Originaljournal und eine unabhängig konfigurierte TrustRoot.
Ein noch aktiver Originalbesitzer sperrt den separaten Leser. Fehlende
Journaldateien, falsche native Journalidentitäten oder fehlende/falsche Schlüssel
werden abgewiesen; der Leser initialisiert keinen Ersatz.

Der Leser unterscheidet den letzten gültigen gespeicherten Zustand vom heute
physisch belegten Zustand. Auch `last_valid_phase: published` ist eine
historische Aussage. `physical_publication_unconfirmed_after_process_loss`
bleibt `true`. `can_dispatch`, `can_activate`, `can_reclaim`,
`native_custody_reconstructible` und `execution_authority` bleiben `false`.
Ein beschädigter letzter Frame kann nur eine als degradiert ausgewiesene ältere
Diagnose liefern. Ein fehlender Slot wird nicht rekonstruiert.

Die Linux-CLI liest diesen Status ausdrücklich:

```text
cowork-runtime-package --publication-status ABS_ORIGINAL_JOURNAL ABS_TRUSTED_KEY KEY_ID PLATFORM
```

Das ist kein Wiederherstellen alter Ausführungs- oder Bereinigungsrechte.
Verlorene ursprüngliche Handles bleiben verloren. Bestehende Auftragsvolumes,
Reservierungen, Runnerdatenbanken und Delivery-/Taskjournale werden durch diese
Komponente weder freigegeben noch migriert. Für eine vollständige Reparatur
bleibt ein neu zugelassener Vorgang mit unabhängiger vollständiger Prüfung,
neuen aktuell gehaltenen Fähigkeiten, sichtbarer Herkunft und erneuter
Qualifikation erforderlich. Aktivierung und Anti-Rollback-Floor bleiben offen.

## Gebundene Prüfungen

Der Komponentenbeleg (development artifact omitted from public sources) trennt
den tatsächlich roten Vorher-Test von den grünen Nachher-Prüfungen. Der alte
gültige CLI-Aufruf veröffentlichte bereits einen vollständig gelesenen
Neun-Rollen-Baum, obwohl das verlangte Journal fehlte. Native Produktionsquellen
waren bei diesem RED unverändert. Quellen, vier tatsächliche Prüfprogramme und
Rohlogs dieses fehlgeschlagenen Produktfalls bleiben separat erhalten.

Der private GREEN umfasst 20 Windows- und 24 Linuxfälle, beide strikten
Paket-Clippy-Prüfungen, Format und je vier Builderfälle. Die sieben bisherigen
Dateisystem-Suites bleiben enthalten. Zwei weitere eigene tmpfs-Suites prüfen
die Journalpflicht sowie den tatsächlichen CLI-/API-Statusleser einschließlich
unabhängiger Schlüssel, aktivem Besitzer und beschädigtem/fehlendem Zustand.

Eine zehnte Suite verwendet ein eigenes vorreserviertes 512-MiB-ext4 mit
4.096 Inodes in einem privaten Mount-Namespace. Ausschließlich ihre neu
gestarteten eigenen Kindprozesse werden an acht Grenzen mit `SIGKILL` beendet:

| Grenze | Gespeicherte Diagnose danach | Neue Veröffentlichung beim Lesen |
| --- | --- | --- |
| vollständige Vorbereitung | `prepared` | keine |
| dauerhafter Intent vor Rename | `intent` | keine |
| ausgeführter Rename | `intent`, physischer Ausgang unbestätigt | keine |
| erster Eltern-`fsync` erfolgreich | `intent` | keine |
| zweiter Eltern-`fsync` erfolgreich | `intent` | keine |
| vollständig bestätigter Abschluss | historisches `published` | keine |
| tatsächliches Intent-`EFBIG` | `prepared`, Dispatch gesperrt | keine |
| tatsächliches Journal-`ENOSPC` | Abweisung des unvollständigen Journals | keine |

Die beiden `fsync`-Grenzen werden an tatsächlich erfolgreichen Kernelaufrufen
des eigenen Testthreads mittels `PTRACE_TRACEME` beobachtet. Es gibt keine
Produktions-Faultschalter. Aufgezeichnete Kindprozess- und Syscallzustände,
unveränderte Statuslesungen, normales Aushängen/Remounten, identische Diagnose
nach Remount, tatsächliches Loop-Detach und Entfernung ausschließlich der eigenen
Fixture werden getrennt geprüft. Siehe die Linux-Beschreibung von
[ptrace(2)](https://man7.org/linux/man-pages/man2/ptrace.2.html) und
[wait(2)](https://man7.org/linux/man-pages/man2/waitpid.2.html).

Diese Probe belegt Prozessabbruch und normales ext4-Remount unter WSL2. Sie
belegt keinen Stromausfall, WSL-Kernelabbruch, Hardware-Persistenz oder native
Linux-Hardwareabnahme. Verwendet werden synthetische signierte Paketdaten und
unsignierte Entwicklungsprogramme. Es gibt weiterhin keine ausgelieferte eigene
WSL-Distribution, keine installierte/aktivierte Runtimeprofilfreigabe und keine
vollständige A–I-/M0–M10-Abnahme.
