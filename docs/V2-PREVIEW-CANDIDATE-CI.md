# Lokaler V2-Buildkandidat

**Aktueller Weg, 10. November 2026:** GitHub Actions ist auf Nutzerwunsch
deaktiviert. Keine GitHub- oder selbst gehosteten Actions-Runner verwenden,
keine Workflows dispatchen und keine Producer-/QA-Run-IDs erfinden.

Der aktive Ablauf steht im [lokalen Releaseweg](V2-PREVIEW-RELEASE.md):
exakten Source-/Befehlsplan prüfen, alle benötigten lokalen Quellgates
tatsächlich ausführen, gemeinsames Frontend genau einmal erzeugen und
unverändert an die nativen release.py-/package.py-Builder geben.
Originalreceipts und Artefakt-SHA256 ersetzen den Actions-Transport.
Build-/Testumfang, Produkt-Signing und vollständige Installed-Acceptance
werden dadurch nicht aufgehoben. Lokale unsignierte Vorbereitung benötigt
keine neu eingerichteten Signingrunner oder Schlüssel.

Die früheren CI-Infrastrukturflags, Environment-/Runnerlabels und
Actions-Artefakt-IDs unten beschreiben ausschließlich den damaligen Weg.

<details>
<summary>Historische signierte Buildkandidaten in GitHub Actions</summary>

# Signierte V2-Buildkandidaten in GitHub Actions

`.github/workflows/preview-candidate.yml` ergänzt den Erzeuger für den bestehenden
Preview-Gate. Der manuell gestartete Workflow baut genau einen vorhandenen
V2-Prerelease-Tag und erzeugt `v2-preview-candidate` als unveränderliches
Actions-Artefakt. Er erstellt keinen GitHub-Release und veröffentlicht keine
Website oder Updatefeeds.

Der mitgelieferte Zustand ist **nicht ausführungsbereit für signierte Releases**:
reale Signieridentitäten, private Profile und zugeordnete Signierrunner sind noch
nicht eingerichtet. `v2-preview-policy.json` bleibt deaktiviert. Der erste Job
bricht bei fehlender Policy oder Infrastrukturkonfiguration ausdrücklich ab.
Es werden keine Schlüssel oder Zertifikate erzeugt.

## Voraussetzungen für den Erzeuger

1. Standardbranch und Release-Tags schützen; keine Änderungen oder Löschungen
   freigegebener Tags zulassen. Diese Ruleset-/Environment-Regeln sind vom
   Betreiber einzurichtende Voraussetzungen; der Workflow prüft deren
   Einrichtung nicht per API. Der Workflow hat ausschließlich
   `workflow_dispatch`, keine PR- oder `pull_request_target`-Ausführung.
2. Die geprüfte Policy auf dem geschützten Standardbranch muss tatsächlich
   aktiviert sein und den realen Updaterschlüsselhash, Preview-Endpunkt,
   Windows-Zertifikatthumbprint und
   `candidate_workflow: ".github/workflows/preview-candidate.yml"` enthalten.
3. Einen vollständig erfolgreichen Lauf von `verify.yml` auf exakt demselben
   Tag-Commit abwarten. Dessen Run-ID ist ein Pflichtparameter. Der Erzeuger prüft
   den Lauf unmittelbar über GitHub und vergleicht Quelle, Tag und `GITHUB_SHA`.
4. Zwei dedizierte selbst gehostete Signierrunner mit den Labels
   `localai-preview-signing-windows-x64` und
   `localai-preview-signing-linux-x64` bereitstellen. Die Runner gehören allein
   dem vertrauenswürdigen Releasebetrieb; dort keine ungeprüften PRs ausführen.
   GitHub-Environment **`v2-preview-signing`** mit erforderlichen Reviewern und
   ausschließlich geschützten Release-Tags konfigurieren.
5. Repositoryvariablen setzen: `V2_PREVIEW_SIGNING_RUNNERS_READY=true`,
   `V2_PREVIEW_WINDOWS_SIGNING_PROFILE` und
   `V2_PREVIEW_LINUX_SIGNING_PROFILE`. Die beiden letzten Werte sind absolute
   Pfade auf bereits vorhandene private Profile des jeweiligen Signierrunners.
   Sie enthalten keine Schlüsselbytes. Die tatsächlichen Profile, Schlüssel,
   Kennwörter und Zertifikate bleiben ausschließlich auf den dedizierten Runnern.

Beide Signierrunner benötigen native Node `22.20.0`, npm `11.6.2`, Rust
`1.94.1`, Git und die jeweiligen nativen Buildwerkzeuge. Python `3.12` wird über
`setup-python` ausgewählt. Windows braucht die MSVC-/SDK-/NSIS-Voraussetzungen,
ein tatsächlich nutzbares Authenticode-Zertifikat und vollständiges natives Perl
für den vendorten OpenSSL-Build. Bei privatem Perl setzt die Runnerumgebung
`OPENSSL_SRC_PERL` auf das geprüfte Programm. Linux benötigt unter anderem
WebKit2GTK 4.1/GTK3, Compiler, OpenSSL-Entwicklungspakete, `patchelf` und die
DEB-/AppImage-Buildvoraussetzungen. Ein Linux-Dateisystem ist erforderlich.
Ein gesetztes Bereitschaftsflag ersetzt keinen verfügbaren Runner: Ein offline
geschalteter oder falsch gelabelter Runner kann weiterhin keinen Kandidaten bauen.

Die vorhandenen Profile werden zuerst gegen die unabhängig geladene Policy
geprüft. Native Paketierung und Signaturprüfung müssen anschließend wirklich
erfolgreich sein. Tests/Clippy werden nicht mit `--fast` übersprungen.
Alle Cargo-Builds laufen mit einem Job. Der gemeinsame Frontendbau verwendet
denselben Tag und wird mit Originalquittung und exakten Ausgabebytes an beide
nativen Jobs übergeben. So bekommen die Pakete denselben Frontendstand.

## Start und Ergebnis

Nach Einrichten der realen Infrastruktur beispielsweise:

```text
gh workflow run preview-candidate.yml --ref v2.0.0-alpha.8 -f verification_run_id=<Run-ID>
```

Der Workflow muss bereits auf dem Standardbranch und auf dem gewählten Tag
vorhanden sein. Ein Dispatch auf einem Branch wird abgewiesen. Das bestehende
Veröffentlichungsrepository bleibt `noshitcoding/LocalAI-Cowork`.

Der Erzeuger archiviert originale signierte Windows-/Linux-Paketquittungen,
NSIS-/DEB-/AppImage-Bytes, Updater-Signaturen, SPDX-SBOMs, gemeinsame
Frontendquittung, getrackte Quellenprovenienz und die tatsächlichen
versionierten Release Notes. Private Signierprofile und Buildcaches werden
nicht als Artefakte ausgewählt. Actions vergibt eine Artefakt-ID und einen
SHA256-Digest; der anschließende Preview-Gate prüft ID, Digest, Erzeugerworkflow
und Commit direkt bei GitHub.

**Buildkandidat und installierte Abnahme sind getrennt.** `acceptance.json`
enthält die tatsächlichen Pakethashes und sämtliche erforderlichen Prüfpunkte
mit `result: "pending"`. `PRODUCER-BUILD.json` meldet deshalb
`installed_acceptance: false` und `release_qualified: false`. Der bestehende
Draft-Gate verweigert die Veröffentlichung dieses unvervollständigen Kandidaten.
Erfolgreiche Paketierung und synthetische Tests werden nicht zu einer erfolgten
Installation oder realen Modellprüfung umgedeutet.

## Originale Abnahme sammeln und neuen Kandidaten finalisieren

Die tatsächlichen Installations-/Produktabnahmen erfolgen mit genau den
unveränderten signierten Paketen des ersten Erzeugerlaufs. Die Originalquittungen
bleiben erhalten. Die Abnahme sammelt `acceptance.json` und Originalbelege unter
`evidence/` gemäß `docs/V2-PREVIEW-RELEASE.md`: sämtliche erforderlichen Checks
beider Plattformen müssen mit `passed`, konkreten Pakethashes und gehashten
Originalbelegen vorliegen. Diesen Stand erzeugt kein Workflow aus synthetischen
Tests oder einer fehlenden Installation.

Für die Sammlung einen dedizierten Runner `localai-preview-acceptance` mit
nativer Node `22.20.0`, Git und GitHub CLI bereitstellen. Das Environment
`v2-preview-acceptance` erfordert Reviewer und geschützte Release-Tags. Die
Repositoryvariablen `V2_PREVIEW_ACCEPTANCE_RUNNER_READY=true` und
`V2_PREVIEW_ACCEPTANCE_DIRECTORY` wählen den vorhandenen absoluten Pfad der
geprüften Originalbelege auf diesem Runner. Die Umgebung benötigt keine
Signierschlüssel. Fehlende Infrastruktur oder noch offene Checks verhindern
die Sammlung.

```text
gh workflow run preview-acceptance.yml --ref v2.0.0-alpha.8 -f candidate_run_id=<Original-Build-Run-ID> -f candidate_artifact_id=<Original-Artefakt-ID>
```

Der Collector lädt den ursprünglichen Kandidaten über dessen exakte Run- und
Artefakt-ID, prüft den GitHub-Digest, den autorisierten Erzeugerworkflow,
Tag-Commit, Signieridentitäten und alle Paketsignaturen. Er kopiert ausschließlich
die ausgewählten Originalbelege und schreibt deren Bindung an diese konkrete
Quelle und dieses konkrete ursprüngliche Artefakt in ein neues unveränderliches
Actions-Artefakt `v2-preview-acceptance`. Änderungen an heruntergeladenen
Kandidaten werden weiterhin zurückgewiesen.

Der abschließende Modus des ursprünglichen Erzeugers ist ein eigener Lauf:

```text
gh workflow run preview-candidate.yml --ref v2.0.0-alpha.8 -f mode=finalize -f verification_run_id=<Run-ID> -f candidate_run_id=<Original-Build-Run-ID> -f candidate_artifact_id=<Original-Artefakt-ID> -f qa_run_id=<Abnahme-Run-ID> -f qa_artifact_id=<Abnahme-Artefakt-ID>
```

Dieser Modus erfordert das geprüfte Environment `v2-preview-release`. Er prüft
beide Original-ZIP-Digests und Run-Herkünfte gegen denselben Tag-Commit. Der
Abnahmelauf muss genau aus `preview-acceptance.yml` stammen. Alle ursprünglichen
Pakete, Signaturen, SBOMs, Frontendbelege und nativen Quittungen bleiben bytegleich;
die offene Abnahme wird durch deren bereits gesammelte Originalbytes ersetzt.
`QA-FINALIZATION.json` hält beide ursprünglichen Run-/Artefakt-Identitäten und
Digests fest. Das Ergebnis ist eine **neue** unveränderliche
`v2-preview-candidate`-Artefakt-ID des zugelassenen Erzeugers. Für
`preview-draft.yml` genau diese neue Run-/Artefakt-ID auswählen. Dessen bestehende
vollständige Quellen-, Signatur-, Abnahme- und Digestprüfungen bleiben wirksam.
Auch die Finalisierung erzeugt keinen Release und behauptet keine bereits
erfolgte Veröffentlichung.

Die Ergänzung dieser Workflows, Infrastrukturhelfer und Anleitung verändert keine eingefrorenen
Rust-/Frontend-Compilerdateien. Sie wird getrennt in die öffentliche
Veröffentlichungshistorie übernommen; frühere lokale Paketbelege werden dadurch
nicht nachträglich erweitert.

</details>
