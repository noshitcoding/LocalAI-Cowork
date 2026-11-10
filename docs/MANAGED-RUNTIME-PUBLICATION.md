# Originale Runtime-Paketwurzel atomar veröffentlichen

`PublicationAttempt` ergänzt die gemeinsame Neun-Rollen-Vorbereitung um einen
Linux-Schritt zur Veröffentlichung eines vollständig geprüften Versionsbaums.
Der Installer legt zuerst eine neue private Wurzel an. Darin entstehen ausschließlich
`package` mit den elf ursprünglichen signierten Eingabedateien und `runtime` mit
allen neun vorbereiteten Rollen. Dieselben bisherigen Staging-, USTAR-,
Reservierungs-, Kopier- und Originalhandle-Prüfungen bleiben verbindlich.

`PublicationPolicy` begrenzt die signierten Eingabearchive und die gemeinsame
Vorbereitung. Das Payloadbudget der äußeren Wurzel begrenzt die Summe aus allen
Komponentenarchiven und allen expandierten regulären Dateibytes. Manifest,
Signatur, Blockrundung, Inodes und Verzeichnis-/Dateisystemmetadaten erfordern
weiterhin die unabhängig begrenzte Verwaltungsablage. Ein zu kleines gemeinsames
Budget wird nach verifiziertem Input und vor der Erzeugung des Runtimebaums
abgewiesen. Die bestehenden Vorreservierungen gelten weiterhin pro Staging- und
Vorbereitungsphase; dies behauptet keine physische Komplettreservierung des
Versionsstores oder eine vom logischen Payloadlimit erzeugte Volumequote.

Die Veröffentlichung verlangt UID 0. Ihr Ziel ist eine unabhängig konfigurierte,
private Management-Verzeichniscapability. Der Paketinhalt bestimmt weder deren
Pfad noch einen Verwaltungsbefehl. Der Zielname ist ausschließlich
`version-<ursprünglicher Manifest-SHA256>`. Ein FD-relatives
`renameat2(RENAME_NOREPLACE)` verschiebt die gemeinsame Wurzel. Bestehende
Versionen bleiben erhalten; es gibt weder Überschreiben noch Copyfallback noch
Cleanup fremder Einträge. Beide ursprünglichen Elternverzeichnisse und alle
Originaldateien bleiben über den Rename hinweg geöffnet. Anschließend werden
beide Eltern mit `fsync` synchronisiert und der gesamte signierte Input,
Rolleninhalt und Namensraum erneut gegen die gehaltenen Originale geprüft.

Das entspricht den getrennten Linux-Garantien: Ein Rename lässt bereits offene
Deskriptoren bestehen; `RENAME_NOREPLACE` verweigert ein belegtes Ziel.
Datei-`fsync` allein persistiert den Verzeichniseintrag nicht; dafür ist eine
eigene Verzeichnissynchronisierung erforderlich. Siehe
[rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html) und
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html).

Vor dem ersten möglichen Rename steht der Versuch im Zustand
`dispatched_unconfirmed` und besitzt bereits die ursprüngliche Zielcapability.
Jeder Fehler nach diesem Punkt erhält diesen Zustand, auch eine Zielkollision.
`publish` beziehungsweise `begin_publication` dürfen ihn nicht noch einmal
ausführen. `observe_publication` kann ausschließlich denselben gehaltenen
ursprünglichen Baum am ursprünglich gebundenen Ziel beobachten und
synchronisieren. Erst danach wird `published` gemeldet. Der getrennte
`begin_publication`-Schritt erlaubt einen erfolgreichen Rename mit noch nicht
beobachtetem Ergebnis; seine bloße Rückkehr liefert keine veröffentlichte
Quittung. Quittungen werden jedes Mal vollständig neu geprüft.

Eine Serialisierung enthält keine native Verwahrung. Es gibt keinen
Konstruktor aus JSON, Pfaden oder alten Identitätsnummern. Nach Prozessende
beziehungsweise Neustart gibt es in diesem Schnitt keine automatische Adoption,
Wiederholung, Aktivierung oder Reparatur. Eine dauerhafte Zustandsführung und
ein ausdrücklich geprüfter Wiederherstellungspfad sind weitere notwendige
Installerarbeiten. Unbestätigte Versuche werden nicht durch neue native Rechte
aus einer alten Statusdatei ersetzt.

Die tatsächliche Linux-CLI führt denselben Dienstpfad aus:

```text
cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID PLATFORM --publish-installation ABS_ATTEMPT_PARENT ATTEMPT_NAME ABS_VERSIONS_PARENT TOTAL_PAYLOAD_BYTES INPUT_BYTES MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES
```

Windows unterstützt die bestehenden Lese-/Staging-Prüfungen; Veröffentlichung
wird dort vor der Vorbereitung mit einem konkreten Linux-Hinweis abgelehnt.
Ein Windows-Host muss künftig seinen eigenen gekoppelten Managementdienst
verwenden. Die CLI installiert keine WSL-Distribution.

Der eigene Main-Prüflauf vom 01.11.2026 bindet die tatsächlich integrierten
zwölf Dateien und 83 Paketquellen. Bei der bewachten Übernahme blieben
4930 weitere erfasste Dateien unverändert. Vier tatsächliche Windows-/Linux-
Prüfprogramme, sämtliche Logs und sieben private Dateisystemfälle sind mit
ihren ursprünglichen SHA-256-Werten zurückgelesen. Der unabhängige Review
des eingefrorenen Kandidaten sowie frühere Staging-, Archiv-, Vorbereitungs-
und RED/GREEN-Nachweise bleiben separat erhalten. Der Main-Lauf prüft
synthetische Testpakete auf tmpfs, keine Veröffentlichung oder Aktivierung
eines echten Runtimeprofils und keine Persistenz nach Stromausfall.

Der gebundene Komponentenbeleg (development artifact omitted from public sources)
enthält 20 Windows- und 24 Linuxfälle, beide strikten Clippy-Prüfungen, Format
und je vier tatsächliche Builderfälle. Alle sechs bisherigen physischen
Fixtures bleiben enthalten. Ein siebtes Fixture verwendet einen eigenen
privaten Linux-Mount-Namespace und ein tmpfs mit 8 MiB und 512 Inodes. Es prüft:

- tatsächlichen Rename derselben ursprünglichen Wurzel, aller neun Rollen und
  ihrer ursprünglichen Inodes; signierte Eingaben und Runtime liegen gemeinsam
  unter dem neuen Namen;
- erfolgreiche, vorerst unbestätigte Dispatchphase, gesperrte Quittung, keine
  zweite Rename-Zulassung und anschließende Bestätigung am Original;
- tatsächliche `EEXIST`-Kollision zweier identischer signierter Pakete, Erhalt
  beider Bäume und kein Retry selbst nach Freigabe des Zielnamens;
- tatsächliches `EXDEV` ohne Kopierersatz, leeres externes Ziel und erhaltener
  ursprünglicher Versuch;
- gemeinsame Input-/Expanded-Grenze vor Erzeugung des Runtimebaums;
- tatsächlichen hashgeprüften CLI-ELF mit anschließendem Lesen aller neun
  Dateiinhalte, Links, Eigentümer und Modi;
- Ablehnung eines bytegleichen Dateiersatzes nach dem Rename, solange dessen
  ursprünglicher Inode weiterhin über den ursprünglichen Versuch gehalten ist.

Normales Aushängen und Entfernen der eigenen Fixturepfade werden getrennt
geprüft. Der tmpfs-Test ist keine Persistenz- oder Stromausfallprüfung. Die
Aufrufe und Zustandsgrenzen benötigen zusätzlich einen dauerhaften ext4-/WSL-
Crashnachweis, bevor der gesamte Installer freigegeben werden kann. Es werden
weiterhin nur synthetische signierte Testarchive und unsignierte
Entwicklungsprogramme verwendet. `installed`, `activated`,
`anti_rollback_applied`, `qualified` und `execution_authority` bleiben `false`.
Reale Werkzeugpakete, Anti-Rollback-Floor, Aktivierung, Update/Rückkehr,
Reparatur, WSL-Bootstrap und die vollständige A–I-/M0–M10-Abnahme bleiben offen.

Der getrennte Folgeschnitt ergänzt ein [verpflichtendes Originaljournal und
einen ausschließlich lesenden Wiederanlauf](MANAGED-RUNTIME-PUBLICATION-JOURNAL.md).
Seine Prozessabbruch-/ext4-Nachweise sind eigenständig gebunden und erweitern
den ursprünglichen Veröffentlichungsbeleg nicht nachträglich.
