# Eingebaute Dokument- und Bildwerkzeuge

Grundbeschreibung: 24.10.2026, 2.0.0-alpha.7-Arbeitsstand, noch nicht paketiert.
Ergänzung vom 02.11.2026: automatische private Windows-Arbeitsablage und
modellfreier Main-Nachweis; Einzelheiten unten.

Aktueller [Implementierungsstand und offene Gesamtpakete](DOCUMENT-MEDIA-IMPLEMENTATION-2026-10-23.md).
Der verbindliche Adapter-/UI-Vertrag liegt unter [DOCUMENT-ENGINE-PROTOCOL.md](DOCUMENT-ENGINE-PROTOCOL.md).

Historischer Audit und ursprüngliche Priorisierung: [Dokumente und Bildbearbeitung vervollständigen](DOCUMENT-MEDIA-COMPLETION-PLAN-2026-10-20.md) (20.10.2026). Die laufende Umsetzung und ihr erweiterter Umfang stehen im oben verlinkten aktuellen Implementierungsstand.

Jeder lokale Chat bekommt ab Werk drei Werkzeuge, sobald die Dokument-Laufzeit eingerichtet ist:

| Werkzeug | Policy | Zweck |
|---|---|---|
| `document_read` | lesend | Text (paginiert), Struktur mit `source_sha256` (`inspect`), Metadaten, gerenderte Vorschau (`preview`), Formatreferenz (`help`) für DOCX, XLSX, PPTX, PDF, Text/TeX und Bilder |
| `document_write` | ändernd | DOCX/XLSX/PPTX aus einer JSON-Spezifikation erstellen (`create`), SHA-gebunden bearbeiten (`edit`), PDFs bearbeiten (`pdf`), PDFs erstellen (`create_pdf`), LaTeX kompilieren (`compile_latex`), mit LibreOffice umwandeln (`convert`) |
| `image_edit` | ändernd | Bildbearbeitung als Schrittfolge: zuschneiden, skalieren, drehen, spiegeln, Helligkeit/Kontrast/Sättigung/Schärfe, Graustufen, Weichzeichnen, Text/Pfeile/Rechtecke, Rand, Überlagerung, Formatwandlung, EXIF entfernen |

Das bisherige native `office`-Werkzeug bleibt unverändert erhalten (Benutzerrechte-Modus, Einzelfreigabe). Sein Katalogeintrag ist byte-identisch, damit gespeicherte Läufe ihre Fingerprints behalten.

## Freigaben und Dateien

- Die Werkzeuge sind **keine** owner-nativen Werkzeuge. Es gibt keinen Benutzerrechte-Modus und keine Checkboxen. Sie erscheinen automatisch, sobald der Chat einen Ordner hat (jeder Chat hat einen Arbeitsordner) und die Engine-Prüfung erfolgreich war.
- `document_read` ist lesend und bleibt auch im Nur-Lesen-Modus verfügbar. `document_write` und `image_edit` folgen dem Zugriffsmodus:
  - „Änderungen bestätigen“ fragt nach.
  - „Autonom“ läuft ohne Rückfrage.
  - „Nur lesen“ verweigert.
- Adressierung wie bei den Dateiwerkzeugen: `{folder_id, path, expected_sha256}`. Ohne `output.folder_id` landet das Ergebnis im Arbeitsordner des Chats.
- **Es wird nie überschrieben.** Jede Ausgabe ist eine neue Datei bzw. bei PDF-`split` ein neuer Ordner. Der Broker veröffentlicht create-only als `FileOperationKind::Generate` mit `DocumentProvenance` (Werkzeug, Vorgang, Engine-Version, Eingaben mit SHA-256, Beleg-SHA). Die Veröffentlichung läuft über dasselbe Protokoll wie andere Dateivorgänge: `in_flight`, `pending_file_operation`, Reconcile nach Abbruch und Wiederherstellen über `operation_after`.
- **Vorprüfung vor der Freigabe:** Zuerst werden Schema, Laufzeit, Eingabe-SHA und „Ziel existiert noch nicht“ geprüft. Was ohnehin scheitern würde, geht sofort als Werkzeugfehler ans Modell, ohne den Nutzer zu fragen.
- Die Freigabekarte zeigt Vorgang, Quellen mit Kurz-SHA, die neue Datei, den Umfang und die Bildschritte (`DocumentToolSummary.tsx`). FileChanges zeigt „Neue Datei erzeugt“ mit Herkunft.
- Agentenfilter dürfen die Werkzeuge nennen. Fehlt die Laufzeit, wird der Chat dadurch eingeengt statt abgelehnt. Würde ein kleines Kontextfenster durch die automatisch ergänzten Werkzeuge überlaufen, entfernt der Preflight sie wieder.

## Ablauf in der Engine

`<Dokumentarbeitsablage>` bezeichnet im Folgenden den beim Dienststart gebundenen
privaten Arbeitsordner. Bei einem geeigneten physischen Pfad ist das weiterhin
`<toolbox>`; andernfalls richtet der Windows-Daemon die unten beschriebene kurze
Arbeitsablage ein. Datenbank, ursprünglicher Datenordner und Benutzerdateien
werden dadurch nicht verschoben.

1. Die Runtime liest die Eingaben über `FileBroker::read_binary` (höchstens 32 MiB, geteilter Lock) und prüft die SHA.
2. `Toolbox::execute_document` legt einen privaten Stage-Ordner `<Dokumentarbeitsablage>/document-stages/<uuid>/` an: `in`, `work`, `work/previews`, `out`, `tmp`, `home`. Die Eingaben werden als Kopien abgelegt und `request.json` geschrieben.
3. Der Adapter (`document_engine.py` bzw. `image_engine.py`, in einem eigenen Digest-Ordner `adapters/documents-<sha>`) läuft als `python -I -X utf8 <adapter> <stage>` mit bereinigter Umgebung, höchstens 2 parallel, insgesamt höchstens 300 s.
4. Rust prüft den Beleg (Format, Exit-Code 0/2, Ausgaben nur unter `out/`, SHA, höchstens 64 MiB und 64 Dateien, Vorschauen als PNG mit höchstens 2 MiB).
5. Ergebnisse veröffentlicht der Broker create-only. Der Stage-Ordner wird gelöscht; Reste räumt der nächste Toolbox-Start weg.
6. LibreOffice (nur `convert` und Office-Vorschauen) startet Rust direkt mit einem frischen kurzen Profil unter `<Dokumentarbeitsablage>/.lo/<UUID>` (22 Zeichen, URL-sicheres Base64 mit allen UUID-Bits), in dem Makros deaktiviert sind, und höchstens 90 s. Die physischen Windows-Pfade für Prozessstart, Office und LaTeX werden vorab geprüft. Bleibt eine Grenze überschritten, wird der Auftrag vor dem Start abgewiesen. Das Profil wird danach entfernt.

Der Vertrag zwischen Rust und Python steht in [DOCUMENT-ENGINE-PROTOCOL.md](DOCUMENT-ENGINE-PROTOCOL.md). Das Vokabular der Spezifikationen pro Format liefert `document_read` mit `operation: "help"`.

## LaTeX und PDF

- **LaTeX läuft nur in Betriebssystem-Isolation.**
  - Der Spike (`.local/document-tools-20261018/TECTONIC-SPIKE.md`) hat gezeigt, dass Tectonic 0.17.0 trotz `--untrusted --only-cached` beliebige Dateien des Nutzers lesen (`\input`, `\openin`, `\includegraphics`, Systemschriften) und per `\openout` sogar schreiben oder überschreiben kann. Eine Prüfung nach dem Lauf reicht dafür nicht.
  - Windows: Jede Kompilierung bekommt ein frisches AppContainer-Profil (ohne LPAC, Win32k erlaubt, keine Capabilities, kein Netz) und ein Job Object (1 Prozess, 2 GiB, CPU-Grenze). Lesen darf sie nur den Quellordner, schreiben nur `work/latex-out`. Tectonic-Ordner und Cache haben dauerhaft eine reine Lesefreigabe für „ALL APPLICATION PACKAGES“.
  - Vor dem Start erhalten die frischen Ausgabeordner vererbbare Rechte für den lokalen Hostbesitzer. Damit bleiben PDF und Log auch bei einem übergeordneten privaten Ordner ohne Rechtevererbung nach Ende des AppContainers lesbar. Zurückgegebene Dateien werden dafür nicht nachträglich mit neuen Rechten versehen; die Prüfung auf gewöhnliche Dateien ohne Hardlinks bleibt aktiv.
  - Linux: Landlock (ABI ≥ 3) plus seccomp (keine Sockets, keine neuen Prozesse). Ohne Landlock bleibt LaTeX aus.
  - `tex_confinement_available()` prüft das mit einem isolierten `tectonic --version`. Erst wenn das gelingt und der vorgewärmte Cache zum Binary passt, meldet der Daemon `latex_available = true`. Die Grenztests und Einzelheiten stehen in `.local/document-tools-20261018/TEX-ISOLATION-NOTES.md` und `docs/WINDOWS-ISOLATION.md`.
  - Grenze unter Windows: Die Kompilierung kann lesen, was jeder AppContainer lesen darf (Windows-Ordner, Programme, Schriften). Nutzerdaten (Dokumente, AppData, Temp) sind nicht lesbar. In Docker ist die Isolation noch nicht getestet; der Standard-seccomp von Docker muss die Landlock-Aufrufe zulassen.
- Ohne LaTeX erzeugt `create_pdf` PDFs über ReportLab aus `spec.markdown`/`spec.blocks` (`options.latex_available = false`). Mit LaTeX nutzt `create_pdf` die eingebaute LaTeX-Vorlage (`latex_prepare` → Tectonic → `latex_finalize`). `compile_latex` braucht immer LaTeX.
- PDF-Bearbeitung (`merge`, `split`, `extract_pages`, `rotate`, `reorder`, `delete_pages`, `watermark`, `fill_form`, `set_metadata`) und das Rendern mit pypdfium2 brauchen kein LaTeX.
- Die Workbench speichert ein versioniertes LaTeX-Projektmanifest mit Hauptdatei und bis zu 15 Begleitdateien als serverseitigen Entwurf. Jede Quelle ist SHA-gebunden. Hauptdateiwechsel und Navigation von Diagnosen zur Projektdatei sind angeschlossen; ein Quellenwechsel verlangt erneutes Erfassen. Größere Projekte und direkte Veröffentlichung bearbeiteter Projektquellen bleiben offen.

## Bildsicht (Vision)

- Jede Modellverbindung hat die Fähigkeit `capabilities.image_input` (Einstellungen → Modelle → „Bildeingabe (Vision)“). Neue Codex-Profile haben sie standardmäßig. Das Feld wird nur gespeichert, wenn es gesetzt ist; bestehende Profil-Hashes bleiben dadurch gleich. Es wird mit dem Profil in jeden Lauf eingefroren.
- Vorschauen (`document_read preview`, `preview: true` bei `document_write`/`image_edit`) speichert der Broker inhaltsadressiert (`retain_preview`, nur PNG bis 2 MiB). Die Tool-Nachricht trägt nur Referenzen (`cowork_images: [{sha256, media_type, page, width, height}]`), nie base64.
- Erst beim Request-Bau lädt `prepare_model` die Bytes und prüft dabei die SHA. `provider.rs` setzt sie nach dem jeweiligen Block von Tool-Ergebnissen als Nutzernachricht ein:
  - OpenAI-kompatibel: `image_url` als Data-URL
  - Ollama: `images[]`
  - Codex: `input_image`
- Die Bilder gelangen nur bei `image_input` in den Request, und nicht bei Raw-Modellen mit `model_binding`. Es werden nur die letzten 2 Stapel mit je bis zu 4 Bildern gesendet; ältere erscheinen als Textplatzhalter. Die logische Bindung und der Fingerprint enthalten nur die Referenzen, die Bytes stehen nur im Wire-Payload. Der Byte-Guard für Codex und die Planungsphase zieht die Bildbytes ab.
- Modelle ohne Bildeingabe bekommen im Werkzeugergebnis den Hinweis, stattdessen Text, `inspect` oder Metadaten zu nutzen.
- Die UI zeigt die Vorschauen im Chat (`ToolPreviews.tsx`). Endpoint: `GET /api/v2/runs/{run}/previews/{sha}`. Er liefert nur Bilder, auf die die Nachrichten dieses Laufs verweisen; der Client prüft die SHA vor der Anzeige.

## Einrichtung

- `tools/toolbox/runtime_install.py --profile documents|full` installiert die gepinnten Wheels aus `requirements-documents.lock`: lxml, openpyxl, Pillow, pypdf, python-pptx, python-docx, pypdfium2, reportlab, xlsxwriter. Dazu kommen Tectonic 0.17.0 (SHA-gepinnt) und ein vorgewärmter TeX-Cache. Chromium/Playwright installiert nur `full`.
- Der Desktop startet das Profil `documents` beim ersten Start automatisch. Docker-Images bringen die Laufzeit mit. Einzelheiten: `tools/toolbox/README.md` und `.local/document-tools-20261018/SETUP-NOTES.md`.
- Der Daemon kennt `programs.tectonic` (Kandidat in `toolbox_config.rs`) und den Laufzeitordner `<data>/tool-runtime` (`Toolbox::configure_document_runtime`). Docker-Images legen die vorgewärmte, schreibgeschützte Laufzeit außerhalb des Datenvolumes ab und setzen dafür `COWORK_DOCUMENT_RUNTIME_DIR` (absoluter Pfad). Die Bereitschaft prüft er im Hintergrund mit `document_engine.py --probe` und dem Tectonic-Warm-Marker.
- `/health` meldet `document_tools_v1`. `/toolbox` meldet `documents_available`, `latex_available`, `office_convert_available`, `document_engine_version` und getrennte Bereitschaftsgründe. Office-Konvertierung verlangt eine erfolgreiche Konvertierungsprobe.

## Windows: automatische private Dokumentarbeitsablage

Der Windows-Daemon prüft beim Start den tatsächlichen physischen Dokumentpfad
einschließlich der benötigten Unterordner für Stages, Office und LaTeX. Reicht
das bestehende Pfadbudget dort nicht aus, verwendet er automatisch
`<KnownFolder_Profile>/.localai-cowork-documents/work/<Locator>`.
`KnownFolder_Profile` ist der nativ von Windows gelieferte Benutzerprofilordner;
`<Locator>` ist die private, fest an den ursprünglichen Toolbox-Ordner gebundene
Kennung. Der Pfad wird nicht aus einer Umgebungsvariablen oder einer Eingabe in
der Oberfläche übernommen. Bei einer Serververbindung betrifft dies den
Windows-Benutzer, unter dem der Daemon läuft.

Die separate Arbeitsablage enthält interne Kopien und temporäre Dateien.
Ursprünglicher Daten- und Toolbox-Ordner, Datenbank sowie Benutzerdateien bleiben
an ihrem bisherigen Ort. Die Dateien werden weiterhin über ihre ursprünglichen
Ordnerrechte und Inhaltsprüfsummen gelesen und ausdrücklich veröffentlicht.
Die private Ablage samt Elternordner ist für gewöhnliche Dateifreigaben gesperrt.

Beim Neustart werden die gespeicherte Quellenbindung und die ursprünglichen
Verzeichnisidentitäten erneut geprüft. Eine kopierte Kennungsdatei oder ein
ersetzter Ordner begründet keine neue Zuordnung. Die gewählte Arbeitsablage
gehört zur Dokumentrevision; vorbereitete Aktionen können dadurch nicht still
auf einen anderen Arbeitsordner umgeleitet werden. Laufende Hintergrundproben
halten die ursprünglichen Sperren bis zu ihrem Ende.

Auch der ausgewählte kurze Pfad muss alle physischen Budgets erfüllen. Eine
kurze Alias-Schreibweise allein reicht nicht. Ist die Ablage ungeeignet oder
ihre ursprüngliche Bindung verändert, wird die Einrichtung beziehungsweise der
Auftrag abgewiesen. Bei überschrittenem Pfadbudget bleibt die Vorabdiagnose
`data_path_too_long` erhalten.
Das hebt keine Windows-Grenze auf und verspricht keine beliebig langen internen
Arbeitsverzeichnisse.

Der modellfreie Main-Nachweis vom 02.11.2026 verwendet einen langen ursprünglichen
Datenpfad und dieselbe kurze Quellenbindung über fünf echte Dienststarts mit
belegtem Ende der jeweiligen Originalprozesse. Er umfasst DOCX, XLSX und PPTX,
echte LibreOffice-Konvertierung und Formelneuberechnung, PDF und Bildbearbeitung,
native OCR, isoliertes LaTeX sowie Browseraktionen und geprüfte Downloads.
Vorbereitung, Bestätigung und die Wiederherstellung einer unterbrochenen
Veröffentlichung werden über Neustarts geprüft. Die Paketprüfungen
haben eigene Nachweise. Dieser Host-Test ist keine Abnahme einer installierten
App, aller Rechner oder einer vollständigen Rechnungsverarbeitung und bewertet
keine Modellgüte.

## Sicherheit und Grenzen

- **Die Engine ist keine Sandbox.**
  - Python und LibreOffice laufen mit den Rechten des Nutzers auf Stage-Kopien. Die Python-Adapter begrenzen Datei-, Netzwerk- und Subprozesszugriffe durch Audit-Hooks. LibreOffice verwendet ein frisches Profil mit deaktivierten Makros. Diese Mechanismen sind keine Betriebssystem-Isolation oder Netzwerk-Sandbox für LibreOffice.
  - Eingaben sind begrenzt: OOXML höchstens 10 000 Einträge und 64 MiB entpackt, DOCTYPE wird abgelehnt, lxml ohne Entities und ohne Netz.
  - Pillow lehnt Bilder über 50 MP ab, Decompression-Bombs gelten als Fehler. Dazu kommen Seiten- und Ergebnisgrenzen.
- Nicht enthalten:
  - Die vollständige Rechnungsworkflow-Kette mit dauerhafter Ergebnisübernahme, Buchung, Ablage und Meldung. Der vorhandene Lese-/OCR- und Prüfablauf ist in [DOCUMENT-EXTRACTION.md](DOCUMENT-EXTRACTION.md) getrennt beschrieben.
  - Team-Läufe, Android/Private-Gäste, Offline-Freigaben und der Geräteagent.
  - Generative Bildbearbeitung und ein WYSIWYG-Editor. Native OCR ist inzwischen angeschlossen und benötigt eingerichtete Sprachdaten; sie ersetzt keine Prüfung der erkannten Angaben.
- Bei Bedarf liefert das Rendern einer Vorschau den Inhalt an das Modell und damit an den gewählten Provider. Das geschieht nur, wenn die Modellverbindung Bildeingabe hat.

## Verifikation

- Rust:
  - `cargo test -p cowork-domain -p cowork-policy`: Argumente, Read-only-Liste, Policy-Matrix 3 × 4
  - `cargo test -p cowork-broker --test generated`: create-only, Ordnerknoten, Reconcile, Aufräumen, Rechte, Grenzen von `read_binary`
  - `cargo test -p cowork-runtime --lib image_tests`: Bildanhänge für OpenAI und Ollama, letzte Stapel, Platzhalter
  - `cargo test -p cowork-runtime --test documents`: Grants, erfolgreiches Nur-Lesen, verweigerte Mutation, Freigabe/Autonom, Nicht-Überschreiben, direkte modellfreie Aktionen, Laufzeitwechsel, Quellenbindung sowie Bild-/PDF-Vorschauen. Mit `COWORK_REQUIRE_DOCUMENT_TESTS=1` muss `COWORK_TEST_DOCUMENT_PYTHON` gesetzt und verwendbar sein.
  - Engine: aktuell 134 ausgeführte Tests (`crates/toolbox/tests/document_engine`); aktuelle Ergebnisse und frühere Isolationsbelege sind im verlinkten Implementierungsbeleg getrennt ausgewiesen.
- UI: `npx vitest run packages/ui/src/DocumentToolSummary.test.tsx packages/ui/src/ToolPreviews.test.ts`, `npx tsc --noEmit`
- Contracts: `npm run contracts:check`
- Engine: siehe `.local/document-tools-20261018/ENGINE-NOTES.md`
- Pflichtqualifikation: `python tools/toolbox/qualify_documents.py --install-tex`; ohne diesen Schalter werden ausdrücklich gesetzte `COWORK_TECTONIC`-/`COWORK_TECTONIC_CACHE`-Pfade in eine neue private Testlaufzeit kopiert.
- HTTP/Neustart: `python tests/system/documents.py --python <Python mit Dokument-Wheels>`; arbeitet ausschließlich mit temporären Daten, ohne Modellprovider.
- Reale Office-Abnahme: zusätzlich `--toolbox-config <private toolbox.json>`; reale isolierte TeX-Abnahme: `--latex` mit ausdrücklich gesetzten Binary-/Cache-Pfaden oder `--tex-config <private toolbox.json>`.
- Browser: zusätzlich `--browser` mit aktuellem Webbuild und Playwright Chromium. Prüft Bildpipeline und Projektentwürfe; mit TeX auch tatsächliche UI-Kompilierung und Fehlernavigation.

## Dateien

- Domain:
  - `crates/domain/src/builtin_tools.rs`
  - `crates/domain/src/documents.rs`
  - `crates/domain/src/model_tools.rs` (`document_tool_catalog`)
- Contracts:
  - `crates/contracts/src/documents.rs`
  - `FileOperationKind::Generate`
  - `ModelCapabilities` in `crates/contracts/src/provider_settings.rs`
- Policy: `crates/policy/src/lib.rs`
- Broker: `prepare_generated`, `GeneratedContent`, `read_binary`, `retain_preview`, `read_preview` in `crates/broker/src/{operations,lib}.rs`
- Toolbox: `crates/toolbox/src/document_engine.rs`, `document_engine.py`, `image_engine.py`
- Runtime:
  - `crates/runtime/src/document_tools.rs`
  - Preflight und Dispatch in `crates/runtime/src/lib.rs`
  - Bilder in `crates/runtime/src/provider.rs`, `model_context.rs` und `codex_gateway.rs`
- Daemon: `services/daemon/src/toolbox_config.rs`, `services/daemon/src/lib.rs` (Health, Preview-Route, Provider-Anlage)
- UI:
  - `packages/ui/src/DocumentToolSummary.tsx`
  - `packages/ui/src/ToolPreviews.tsx`
  - `packages/ui/src/FileChanges.tsx`
  - `packages/ui/src/ProviderSettingsPanel.tsx`
  - `packages/ui/src/i18n.ts`
