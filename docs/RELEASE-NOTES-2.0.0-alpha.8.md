# LocalAI Cowork V2 · 2.0.0-alpha.8

**Status: Releasevorbereitung, noch nicht öffentlich veröffentlicht.**
Stand der Beschreibung: 9. November 2026. Eine Veröffentlichung muss diese
Notizen um die tatsächliche Quellrevision, Paketnamen, Prüfsummen, Signierung,
Updatekonfiguration und Abnahmebelege ergänzen. Diese Datei selbst ist kein
bestandener Releasebeleg.

## Neu gegenüber dem bisherigen V2-Previewstand

- Workflows mit typisierten Eingaben und verbundenen Schrittergebnissen,
  Bedingungen, Zweigen, relativen Wartezeiten, eingefrorenen Unterabläufen,
  Listenverarbeitung und ausdrücklich bestätigten Prüfformularen.
- Dokument- und Bildaktionen auf bestätigten Arbeitskopien mit bewusster
  Enginewahl und Freigaben; native OCR für Deutsch/Englisch und typisierte
  Tabellenaktionen sind im aktuellen Quellstand integriert.
- Quellenansicht, Extraktionsstarter und geführte Dokumentauswahl mit gemeinsam
  gebundenem Dateipfad und Hash sowie Windows-Pfadvorprüfung vor dem Start.
- Themenordner und Unterordner in Projektwissen/persönlichem Wissen,
  Notizen/Importe und Graphen aus Tags und Quellen.
- Getrennte Runtime-Ergebnisversionen, frische Leserechte, Ergebnisansicht und
  ausdrücklicher Ergebnisabruf sind im Quellstand ergänzt; der vollständige
  verwaltete Betriebsablauf bleibt experimentell.
- Arbeitsbereiche werden bei Bedarf nachgeladen; gemeinsam genutzte statische
  Vertragsvalidatoren und getrennte Ansichten verringern den anfänglichen
  JavaScript-Download. Die lokalen Browserprüfungen beziehen sich auf den
  aktuellen Frontendbau, nicht auf eine bereits installierte Releaseversion.
- Scheitert das Nachladen eines Arbeitsbereichs, bleiben Chat und Entwurf
  erhalten. Eine ausdrückliche Wiederholung lädt den betroffenen Bereich neu;
  der Fehler- und Wiederholungsweg wurde im lokalen Browser geprüft.
- Gezielte Vite-/Vitest-/fast-uri-Aktualisierungen härten den V2-Frontendbau.
  Die getrennte V2-Abhängigkeitsprüfung meldet am eingefrorenen Stand keine
  bekannten npm-Auditfunde; Enterprise wird unabhängig geprüft.
- OpenID Connect 4.1.0 ersetzt im Core-Lock crypto-bigint 0.5.5 durch
  0.7.11. Der Core-RustSec-Lauf vom 11. November meldet weiterhin RSA 0.9.10
  und vier Unmaintained-Hinweise; der frühere Desktopbericht bleibt historisch.
  Details und Prüfgrenzen: [Abhängigkeitssicherheit](DEPENDENCY-SECURITY.md).
- Für die Rust-Abhängigkeit glib 0.18.5 enthält der Desktop den lokalen
  Zweizeilen-Backport des VariantStrIter-Out-Pointer-Fixes. Vier zusätzliche
  Fälle prüfen echte GLib-Iteration und gültig gehaltene String-Borrows;
  Resolver- und Testprofilnachweise sind gesondert zu prüfen. Version 0.18.5
  bleibt im veröffentlichten Scannerbereich, ohne Advisory-Ignore oder
  Sicherheitsfreigabe. Originale MIT-/COPYRIGHT-Hinweise und Quellprovenienz
  sind in der Paketkonfiguration unter `licenses/glib/` gebunden; daraus folgt
  kein tatsächlicher Paket-, Installations- oder Release-PASS.
- Der Preview-Veröffentlichungsweg bindet Quellstand, Paketbelege,
  Signieridentitäten und originale Build-/Abnahmeartefakte an ihre Prüfsummen
  und Erzeugerläufe. Fehlende Signierung oder installierte Abnahme wird
  dadurch nicht zu einem bestandenen Releasegate.

Die genaue Abgrenzung steht im [Previewumfang](PREVIEW-SCOPE-2.0.0-alpha.8.md).
Ein implementierter Quellpfad und seine Einzeltests sind keine vollständige
Installed-App-, Mehrgeräte-, Enterprise- oder Produktionsabnahme.

## Paketziele

Windows x64: NSIS, aktueller Benutzer, getrennte V2-Previewkennung.
Linux x86_64: DEB und AppImage; glibc **ab 2.39**, GTK 3 und WebKitGTK 4.1.
RPM, macOS, ARM64, musl-only Linux und Android sind keine Paketziele dieses Releases.
Nur Plattformen mit bestandenem tatsächlichem Paketgate dürfen angeboten werden.

## Bekannte Grenzen

- Managed-Runtime-/Dienst-/Profilintegration, entfernte Ergebnisübernahme und
  installierte App-Abnahme bleiben in ihrem dokumentierten Umfang offen.
- Vollständige Rechnungs-/OCR-/Buchungskette, absolute Workflowtermine,
  Ereigniswarten, automatische Seitennachladung und dauerhafte Übernahme
  werden nicht als fertig angekündigt.
- Windows-Dokumentpfade werden vorgeprüft; vollständige Langpfadunterstützung
  und der gesamte Linux-Dokumentablauf bleiben getrennte Abnahmeaufgaben.
- Semantische Entitätsextraktion, Android-/Mehrgeräte-/IdP-/Cluster-/Office-
  Betrieb und Enterpriseproduktion sind keine vollständigen Previewzusagen.
- Signierung und automatische Updates sind erst mit zum Paket passenden
  Schlüsseln, Metadaten und realer Update-/Recoveryprüfung zugesichert.
  Ein automatisches V1-auf-V2-Upgrade wird nicht angeboten.

## Installation und Daten

V1 sichern und behalten, ein separates V2-Profil verwenden und zunächst auf
Dateikopien arbeiten. V2 niemals auf eine V1-Datenbank zeigen lassen.
Historienimport und bestätigte aktuelle Inhaltsübernahme sind getrennt;
alte Credentials, Rechte, Runs, Freigaben, Zeitpläne und Outboxeinträge
werden nicht aktiviert. V1 mit seinem bestehenden Datenbestand bleibt der
Rückweg; kein Downgrade einer V2-Datenbank nach V1.

Das vorhandene öffentliche [V1-Release v0.3.1](https://github.com/noshitcoding/LocalAI-Cowork/releases/tag/v0.3.1)
bleibt erreichbar. Das reservierte
[V2-Releaseziel v2.0.0-alpha.8](https://github.com/noshitcoding/LocalAI-Cowork/releases/tag/v2.0.0-alpha.8)
ist erst nach tatsächlicher Veröffentlichung ein Download.

## Vor Veröffentlichung zu vervollständigen

| Nachweis | Voraussetzung |
| --- | --- |
| Quellrevision | Eindeutiger Release-Commit und dazu konsistente App-/Buildversionen. |
| Quellgate | Frische Frontend-/Vertrags-/Typ-/Build- und Rustprüfungen am eingefrorenen Stand. |
| Pakete | Tatsächliche Dateinamen, SHA256, Paket-/Sidecarbelege und Softwarestückliste. |
| Signierung und Updates | Ehrlicher Signierstatus, Kanal-/Schlüsselbindung, tatsächliche Update-/Recoveryergebnisse. |
| Installation | Neuinstallation und aktueller GUI-/Modell-/Werkzeug-/Datei-/Workflow-/Stop-/Neustartweg je angebotener Plattform. |
| Öffentliche Seiten | DE/EN, Linuxanforderungen, Previewgrenzen, Releaseziele und aktuelle oder ausdrücklich historische Screenshots. |

Historische Berichte und frühere Installer bleiben an ihre damaligen Quellen
gebunden und dürfen diese noch fehlenden Nachweise nicht ersetzen.
