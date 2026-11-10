# Gemeinsame Vorbereitung aller neun Runtime-Paketrollen

`StagedPackage::inspect_installation` prüft jetzt alle neun Archive desselben
ursprünglichen signierten Pakets. `InspectedInstallation` hält den gesamten
Paketkontext und sämtliche ursprünglichen Eingabehandles. Einzelarchiv und
Gesamtpaket verwenden denselben USTAR-Prüfer, denselben Kopierpfad und dieselben
Zielprüfungen. Es gibt keinen zweiten Entpacker mit abweichenden Sicherheitsregeln.

Die Rollenreihenfolge bleibt durch das signierte Manifest gebunden: Basis-Rootfs,
Browser, containerd, Dokumentwerkzeuge, Git, gVisor, Node, Python und Runtime-Dienst.
Jede Quittung nennt das tatsächlich gehaltene Manifest samt Hash und für jede
Rolle den signierten Komponentenhash. Reine Quittungsdaten können keine native
Verwahrung oder Installationsautorität erzeugen.

`InstallationLimits` begrenzt die Summe aller Archive auf höchstens 200.000
Einträge, 32 GiB expandierte Dateibytes und 16 MiB Pfad-/Linktext. Einzelne
Komponenten behalten die vorherigen Grenzen. Die Rechnung verwendet geprüfte
Addition und Subtraktion. Eine Rolle kann nicht neunfach das Gesamtbudget
beanspruchen. Selbst ein ungültiges letztes Rollenarchiv verhindert die gesamte
Vorbereitung; zuvor entsteht kein erfolgreicher Teilstand.

Unter Linux verlangt `prepare` UID 0 und eine neue private Zielwurzel auf dem
unabhängig begrenzten Verwaltungsdateisystem. Zuerst werden alle benötigten
Zieldeskriptoren gemeinsam reserviert. Der temporäre Bedarf umfasst die gelockten
zwei Deskriptoren je aktivem Verzeichnisiterator, die Zielwurzel, Helferraum und
zwei zusätzliche gehaltene Bereichsdeskriptoren für jede der neun Rollen.
Weder Bibliothek noch CLI erhöhen stillschweigend Prozess- oder globale Limits.

Danach entstehen die festen privaten Rollenverzeichnisse `component-0` bis
`component-8`. Der Kern reserviert **alle regulären Dateiinhalte aller Rollen
physisch, bevor die erste Nutzlast in irgendeine Rolle kopiert wird**. Erst nach
allen Kopien folgen Linkerstellung und Verzeichnisschutz. Die ursprünglichen
Datei-, Verzeichnis- und Linkhandles bleiben weiterhin gehalten. Das Dateisystem
muss zusätzlich Blockrundung, Verzeichnis-/Inodemetadaten und die signierten
Eingabearchive begrenzen; das Nutzdatenbudget allein ist keine Volumequote.

`PreparedInstallation` entsteht erst nach erfolgreicher vollständiger Prüfung
aller neun Rollen, der unveränderten Quelle und des gesamten Zielnamensraums.
Seine Felder sind privat; es gibt weder `Clone` noch `Deserialize` oder einen
öffentlichen Konstruktor aus Pfaden, Hashes oder Rollenlisten. `verify()` prüft
die ursprünglichen Quellen und alle gehaltenen Ziele erneut. Ein ersetztes
Zielobjekt, ein fremdes Mitglied oder eine manipulierte Datei sperrt den Stand.
Bei Fehlern bleiben begrenzte, unqualifizierte Arbeitsstände erhalten; kein
vorheriger Stand oder unbekannter Pfad wird gelöscht oder übernommen.

Die native CLI stellt die beiden getrennten Schritte ausdrücklich bereit:

```text
cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID PLATFORM --stage ABS_PRIVATE_PARENT STAGE_NAME INPUT_BYTES --inspect-installation TOTAL_ENTRIES TOTAL_EXPANDED_BYTES TOTAL_PATH_BYTES
cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID linux_x86_64 --stage ABS_PRIVATE_PARENT STAGE_NAME INPUT_BYTES --prepare-installation ABS_PRIVATE_TARGET OUTPUT_NAME TOTAL_ENTRIES TOTAL_EXPANDED_BYTES TOTAL_PATH_BYTES
```

Die Quittung unterscheidet `all_roles_inspected` und `all_roles_prepared`.
`installed`, `qualified`, `execution_authority` und
`native_custody_reconstructible` bleiben `false`. Dies ist ein gemeinsamer
Vorbereitungsstand für den Installer. Atomare Versionsveröffentlichung,
Anti-Rollback, Aktivierung, Reparatur, Rückkehr und die eigene WSL-Distribution
werden damit noch nicht behauptet.

Der eigene Main-Prüflauf vom 01.11.2026 bindet die tatsächlich integrierten
14 Dateien und 80 Paketquellen. Bei der bewachten Übernahme blieben 4.911
weitere erfasste Dateien unverändert. Vier tatsächliche Windows-/Linux-
Prüfprogramme, sämtliche Logs und sechs private Dateisystemfälle sind mit
ihren ursprünglichen SHA-256-Werten zurückgelesen. Der unabhängige Review
des eingefrorenen Kandidaten sowie frühere Staging-, Archiv- und RED/GREEN-
Nachweise bleiben separat erhalten. Dieser Stand bereitet synthetische
signierte Neun-Rollen-Pakete vor; er installiert keine WSL-Distribution.

Der aktuelle gebundene Nachweis (development artifact omitted from public sources)
umfasst 20 Windows- und 23 Linuxfälle, beide strikten Paket-Clippy-Prüfungen und
je vier Builderfälle. Die vier bisherigen tatsächlichen Linux-Fixtures für
`ENOSPC`, Einzelarchiv-/CLI-Extraktion, Originalverwahrung und tiefe Bäume bleiben
enthalten. Zwei zusätzliche private Linux-Fixtures prüfen:

- Vollständige Vorbereitung aller neun Rollen über API und den tatsächlichen,
  hashgeprüften CLI-ELF; alle Zielbytes, Dateirechte und Links werden geprüft.
  Ein bytegleicher Ersatz der letzten Rolle wird abgelehnt. Ein eigener
  CLI-Unterprozess mit `RLIMIT_NOFILE=100` wird vor dem ersten Rollenverzeichnis
  abgelehnt; das Elternlimit bleibt unverändert.
- Tatsächliches `ENOSPC` bei der Reservierung einer 9-MiB-Datei in der letzten
  Rolle auf einem eigenen Dateisystem mit 8 MiB. Sämtliche bis dahin erzeugten
  regulären Dateien aller früheren Rollen enthalten ausschließlich Nullbytes.
  Kein Rolleninhalt wurde vor Abschluss der Gesamtreservierung kopiert.

Jedes Fixture besitzt einen eigenen privaten Mount-Namespace. Die positive
Neun-Rollen-Probe begrenzt Inodes auf 256, die übrigen auf 64; alle begrenzen
Dateibytes auf 8 MiB. Reguläres Aushängen und Entfernung der eigenen Fixturepfade
werden gesondert belegt. Die Pakete und Signierschlüssel sind synthetische
öffentliche Testdaten. Diese Komponentenprüfungen liefern keine realen
Werkzeugpakete und qualifizieren weder installierte App/Runtime noch native
Linux-Hardware, Gesamtbetrieb oder den vollständigen A–I-/M0–M10-Umbau.

Der getrennte nächste Schritt zur [atomaren Veröffentlichung einer gemeinsamen
Originalwurzel](MANAGED-RUNTIME-PUBLICATION.md) behält Input und alle Rollen
zusammen. Er ersetzt keine Aktivierung, Neustartwiederherstellung oder Freigabe
einer installierten Runtime.
