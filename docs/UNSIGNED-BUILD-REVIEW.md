# Unsigned native package review

`preview-build-review.yml` baut nach einem vollständig erfolgreichen `verify.yml`
auf demselben Commit einen prüfbaren Windows-/Linux-Paketsatz. Der manuelle Lauf
verlangt die vorhandene Repositoryvariable `LOCALAI_COWORK_ALLOW_UNSIGNED_RELEASE=true`, die
exakte 40-stellige SHA des dispatchten Commits und die erfolgreiche Verify-Run-ID.
Branchname und ein alter grüner Lauf reichen nicht.

```text
gh workflow run preview-build-review.yml --ref <Branch-oder-Commit> -f source_commit=<40-stellige-SHA> -f verification_run_id=<Run-ID>
```

Der Workflow muss auf dem Standardbranch für den manuellen Start registriert
und im ausgewählten Commit vorhanden sein. Ein gemeinsam gebautes Frontend
wird über Artefakt-ID, Quellmanifest, Originalquittung und Ausgabebytes an beide
native Jobs gebunden. Node 22.20.0, npm 11.6.2 und Rust 1.94.1 sind festgelegt.
Die bestehenden Helfer `release.py`, `frontend.py` und `package.py` erzeugen
NSIS unter Windows sowie DEB und AppImage unter Ubuntu 24.04. Native
Desktop-Format-/Test-/Clippyprüfungen laufen vollständig; `--fast` wird nicht
verwendet. Die Paketquittungen, Originalfrontendquittung, Sidecar-Cargobeleg,
SHA256-Pins, begrenzte native Buildlogs und SPDX-Stücklisten liegen bei den
neuen Actions-Artefakten.

Das gemeinsame Frontend-Artefakt bleibt sieben Tage, die nativen Paketsätze
14 Tage verfügbar. `UNSIGNED-BUILD-REVIEW.json` hält `signed`, `installed`,
`published` und `release_qualified` ausdrücklich auf `false`. Der Lauf führt
keinen Installer aus und verändert keine Tags, Releases, Updatefeeds oder Pages.
Für den vollständigen Release fehlen weiterhin Signierung und die Abnahme der
tatsächlich installierten Paketbytes; der bestehende signierte Preview-Gate
und seine deaktivierte Vertrauenspolicy werden nicht verändert.

Die Builds laufen ausschließlich auf kurzlebigen GitHub-Runnern. Windows nutzt
ein neues kurzes Arbeitsverzeichnis mit privatem TEMP und prüft vorhandenes
natives Windows-Perl samt Standardmodulen sowie x64-MSVC. Tauri stellt seine
gebundenen Bundlewerkzeuge bereit. Linux führt die Desktop-Prüfungen unter
Xvfb aus, installiert GTK/WebKit-/Bundlevoraussetzungen und darf nur auf dem
GitHub-gehosteten Linux-Runner das unbenutzte feste Android-SDK-Verzeichnis
freigeben. Auf einem Benutzerrechner ist keine Bereinigung vorgesehen.
