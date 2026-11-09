# V2 preview: prüfbarer Veröffentlichungsweg

Zielversion dieses Arbeitsstands ist `2.0.0-alpha.8`. Eine öffentliche Preview
bleibt ein GitHub-Prerelease. Die reguläre Freigabe in `release-gates.json` bleibt
gesperrt, solange ihre eigenen Nachweise fehlen. Historische alpha.7-Pakete und
deren Installationsbelege gelten nicht für diese neue Version.

## Voraussetzungen

- Vollständiger geprüfter V2-Commit im Veröffentlichungsrepository; sauberer
  Arbeitsbaum und vorhandener Tag `v2.0.0-alpha.8` auf genau diesem Commit.
  Commit, Push und Tag-Veröffentlichung sind gesonderte Operatoraktionen.
- Erfolgreicher aktueller Lauf von `.github/workflows/verify.yml` auf demselben
  Commit. Der Preview-Gate fragt den Lauf und alle Jobs unmittelbar über GitHub ab.
  Beide Plattformen müssen Core-, Desktop-, Releasewerkzeug-, Dokument-, Office-
  und Dokumentoberflächentests bestanden haben. Fehlende/übersprungene Jobs sperren.
- Native Windows-x64-NSIS- sowie Linux-x64-DEB-/AppImage-Pakete aus diesem Stand,
  mit unveränderten Sidecar-, Frontend- und Paketquittungen. `--fast` reicht für
  eine Veröffentlichung nicht. Windows-Installerressourcen, Paketversion und
  Originalhash müssen übereinstimmen.
- Ein ausdrücklich ausgewähltes privates Signierprofil für
  `tools/desktop/package.py --signing-profile <absoluter privater Pfad>`.
  Das Profil enthält Format `1`, `updater_public_key`, `updater_endpoint`,
  `signing_private_key_file`, optional `signing_password_file` sowie unter Windows
  `windows_certificate_thumbprint` und `timestamp_url`. Schlüsseldateien und
  Kennwörter gehören außerhalb des Repositorys; sie werden nicht eingecheckt,
  als Buildartefakt hochgeladen oder in öffentliche Quittungen übernommen.
- Tatsächlich gültige Authenticode-Signierung unter Windows; der Paketbauer
  prüft das ausgewählte Zertifikat am Desktop und am Installer. Tauri-Updater-
  Signaturen werden beim Staging zusätzlich kryptographisch gegen den originalen
  öffentlichen Schlüssel geprüft: Ed25519 über BLAKE2b-512 und die signierte
  Kommentarbindung. SPDX-2.3 muss sämtliche angebotenen Paketbytes abdecken.
- Unabhängig geprüfte Freigabepolicy auf dem geschützten Standardbranch:
  `.github/v2-preview-policy.json` bindet Repository, SHA256 des tatsächlichen
  Tauri-Updater-Public-Key-Strings, exakten Preview-Endpunkt, tatsächlichen
  Windows-Zertifikatthumbprint sowie den genehmigten Kandidaten-Producerworkflow.
  Der mitgelieferte Stand ist absichtlich `enabled: false` mit leeren Identitäten;
  er muss mit realen Werten separat geprüft werden. Eine Paketquittung darf ihren
  eigenen Signierer nicht zur Vertrauenswurzel erklären.
- Aktuelle installierte Abnahme sämtlicher angebotenen Pakete in wegwerfbaren
  Profilen einschließlich Sicherung/Wiederherstellung und V1-Profilisolation.
  Der Gate installiert nichts und greift nicht auf reale Benutzerprofile zu.

Die Standardeinstellungen der App enthalten bewusst keinen erfundenen
Update-Schlüssel oder Endpunkt. Ein signiertes Preview-Profil verwendet einen
eigenen HTTPS-Pfad mit dem Suffix **`/updates/v2/preview.json`**. Beide Plattformen
müssen denselben Updaterschlüssel und Endpunkt verwenden. Der Feed verweist auf
die exakte Tag-Downloadadresse, niemals auf einen beweglichen `latest`-Alias.

## Kandidat vorbereiten

Alle folgenden Ausgaben sind neu anzulegen. Bestehende Belege werden erhalten.
Der Kandidat darf beispielsweise unter dem ignorierten `.local/` liegen.

```powershell
$candidate = [IO.Path]::GetFullPath('.local/alpha8-candidate')
New-Item -ItemType Directory -Path $candidate
python tools/release/preview.py snapshot --tag v2.0.0-alpha.8 `
  --output "$candidate/source-provenance.json"
```

Der Snapshot bindet sämtliche gewöhnlichen getrackten Dateien, den Commit,
Version und Tag. Nicht eingecheckte Quellen und ein fehlender/falscher Tag führen
zum Abbruch. Er wird vor dem gebundenen Frontend-/Sidecar-/Paketbau erzeugt.
Für das Frontend dient `tools/desktop/frontend.py`, für den nativen Sidecar
`tools/desktop/release.py` und für den Paketbau `tools/desktop/package.py`.
Linux baut nativ auf einem Linux-Dateisystem; Windows baut nativ unter Windows.
Unterschiedliche Frontendbytes zwischen den Plattformen benötigen zuerst eine
bewusst abgeglichene gemeinsame Frontendlieferung.

Der fertig vorbereitete Kandidat hat folgende Struktur; private Profile,
Zugangsdaten und Benutzerdateien sind kein Bestandteil:

```text
candidate.json
source-provenance.json
frontend/receipt.json
windows/receipt.json, Installer.exe, Installer.exe.sig, sbom.spdx.json
linux/receipt.json, Paket.deb, Paket.AppImage, Paket.AppImage.sig, sbom.spdx.json
acceptance.json
evidence/… Originalprotokolle der installierten Paketprüfungen
RELEASE-NOTES.md
```

`candidate.json` wählt ausschließlich Pfade innerhalb dieses Verzeichnisses:

```json
{
  "format": "localai-cowork-preview-candidate/1",
  "frontend_receipt": "frontend/receipt.json",
  "packages": {
    "x86_64-pc-windows-msvc": "windows",
    "x86_64-unknown-linux-gnu": "linux"
  },
  "acceptance": "acceptance.json",
  "notes": "RELEASE-NOTES.md"
}
```

Die installierte Abnahme verwendet Format
`localai-cowork-preview-installed-acceptance/1`, `version`, `commit` und
`source_sha256` aus dem Snapshot. `platforms` enthält für beide Hostnamen:

- `receipt_sha256`: Hash der unveränderten ursprünglichen Paketquittung;
- `packages`: Objekt aus Bundleart (`nsis` oder `deb` und `appimage`) und
  tatsächlichem SHA256 genau des installierten Pakets;
- `checks`: Für **jeden** unten genannten Prüfpunkt ein Objekt mit
  `result: "passed"`, `evidence` (relativer Originalprotokollpfad) und dessen
  `sha256`. Fehlende, leere oder veränderte Protokolle sperren die Veröffentlichung.

Erforderliche Prüfpunkte sind `installed_start`, `first_setup`, `model_connection`,
`chat_tool_approval`, `file_import_download`, `workflow`, `cancel`,
`restart_recovery`, `backup_restore`, `fresh_profile` und `v1_profile_isolation`.
Die Originalprotokolle müssen die tatsächlichen Schritte/Ergebnisse für jedes
angebotene Paket beschreiben. Testprovider allein qualifizieren keine reale
Modellverbindung. Vorhandene historische Nachweise werden nicht umetikettiert.
Die veröffentlichten Metadaten enthalten nur die geprüfte Abdeckung und Hashes;
die privaten Originalprotokolle bleiben im zugriffsbeschränkten Kandidaten.

## Staging und GitHub-Entwurf

Der lokale Gate benötigt Python 3.12, Node 22.20.0, Git sowie authentifiziertes
`gh` mit Leserechten für den ausgewählten Verification-Lauf:

```powershell
python <geprüfter-Toolpfad>/preview.py fetch --source-root <V2-Quellcheckout> `
  --tag v2.0.0-alpha.8 --repository noshitcoding/LocalAI-Cowork `
  --trusted-policy <unabhängig-geprüfter-absoluter-Policypfad> `
  --candidate-run-id <Producer-Run-ID> --candidate-artifact-id <Artefakt-ID> `
  --archive <neue-absolute-ZIP-Datei> --output <neuer-absoluter-Kandidatenordner>

python tools/release/preview.py stage --tag v2.0.0-alpha.8 `
  --repository noshitcoding/LocalAI-Cowork --ci-run-id <Run-ID> `
  --trusted-policy <unabhängig-geprüfter-absoluter-Policypfad> `
  --candidate-run-id <Producer-Run-ID> --candidate-artifact-id <Artefakt-ID> `
  --archive <digest-geprüfte-originale-ZIP-Datei> `
  --candidate <entpackter-absoluter-Kandidatenordner> `
  --output ([IO.Path]::GetFullPath('.local/alpha8-public'))
```

Der Producerlauf muss zum konfigurierten Workflow im selben Repository gehören,
erfolgreich beendet sein und exakt den Release-Commit verwendet haben. Die
explizite Artefakt-ID muss zu diesem Lauf gehören und einen nicht abgelaufenen
SHA256-Digest liefern. `fetch` prüft die heruntergeladene ZIP gegen diesen Digest,
bevor es ausschließlich gewöhnliche, begrenzte Dateien entpackt. `stage` prüft
dieselben Live-Metadaten erneut und bindet sämtliche extrahierten Dateien an die
Original-ZIP. Änderungen vor oder während des Stagings sperren die Freigabe.

Erst nach allen Prüfungen entstehen ein neues öffentliches Paketverzeichnis,
`SHA256SUMS`, getrennte Plattform-SBOMs, `source-provenance.json`,
`release-provenance.json`, ein lokaler `preview.json`-Feed und `release-plan.json`.
Maschinenpfade und private Paket-/Abnahmeprotokolle werden nicht übernommen.
Ein späterer Bytewechsel verhindert die Entwurfserstellung.

`draft_preview.py --staged <absoluter Pfad>` zeigt standardmäßig nur den
geprüften Befehl. Der explizite Schalter `--create-draft` legt ausschließlich einen
GitHub-Entwurf mit `--draft --prerelease --latest=false --verify-tag` an.
Zum Erstellen ist zusätzlich `--trusted-policy <absoluter Policypfad>` nötig;
Policy, Producer und der aktuelle Remote-Tag werden unmittelbar erneut geprüft.
Der Remote-Tag muss weiterhin auf demselben Commit stehen. GitHub-Regeln müssen
Änderungen/Löschen der freigegebenen Release-Tags auch zwischen den API-Aufrufen
verhindern.
Bestehende Assets werden nicht überschrieben. Reguläre V1-/Stable-Releases,
GitHub Pages und der gehostete Updatefeed bleiben dabei unverändert.

Alternativ bietet `.github/workflows/preview-draft.yml` einen manuellen Lauf:
Tag, explizite Kandidaten-Artefakt-ID aus einem ausgewählten Actions-Lauf im selben Repository
und erfolgreicher Verification-Lauf sind ausdrücklich anzugeben. Vor Einsatz
die GitHub-Environment **`v2-preview-release`** mit erforderlichen Reviewern
konfigurieren und den Standardbranch schützen. Der Workflow lädt Releasewerkzeuge
und Policy getrennt vom Standardbranch; der gewählte Tag liefert ausschließlich
den zu prüfenden Quellstand. Producerlauf, Artefakt-ID/-Digest und Policyhash stehen
in der öffentlichen Provenienz. Nur die geprüfte öffentliche Ausgabe wird als neues Artefakt
gespeichert und als Prerelease-Entwurf hochgeladen. Der Kandidat mit privaten
Prüfprotokollen muss bereits vor seinem Upload auf vertrauliche Daten geprüft sein.

Das abschließende Veröffentlichen des geprüften Entwurfs sowie der getrennte
Website-/Previewfeed-Wechsel sind eigene Operatoraktionen. Dabei Downloadlinks
und Plattformumfang mit den tatsächlichen Releaseassets abgleichen. Das reguläre
`release-gates.json` wird durch den Previewweg nicht freigeschaltet.
