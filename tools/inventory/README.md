# Windows program artifact inventory (partial M0)

`windows_runtime_inventory.py` reads only fixed, named program artifacts under explicitly
selected `Local AI Cowork` and/or `Open Cowork` installation directories. It reads the
matching HKCU 64-bit uninstall metadata (`DisplayName`, `DisplayVersion`,
`InstallLocation`, `Publisher`), checks that the explicit directory matches the registry,
preserves NSIS's raw quoted installation-directory value and normalizes exactly one
balanced outer quote pair without interpreting a command line,
and parses PE headers and fixed file/product versions directly from held file bytes.
It does not load or launch those binaries, enumerate the installed directories, extract
installed archives, select SQLite/user data/credential files, or install/repair anything.

The Local AI Cowork allowlist includes the application, WebView loader, Python/Crew
bundle, Codex bundle, local daemon/PDFium bundle, and `runtime-layout.json`.
Open Cowork is limited to `app.exe`; no matching original 0.1.8 source capture is implied.
Missing expected files are recorded explicitly. In particular, a missing
`runtime-layout.json` remains missing; no replacement layout is inferred or written.

Every existing artifact is held concurrently through the unchanged qualified
`tools/migration/v1_sqlite_export.py` native deny-write/delete handles. All existing
explicit artifact ancestors and installation namespaces also have native no-delete
fences. Reparse points, hardlinks, duplicate physical identities, pre-existing writers,
nonlocal paths, namespace/presence drift, and registry drift are refused. Missing-file
absence is an observation before and after, rather than proof that it never appeared
transiently. SHA256, size, volume and physical file identity are recorded before and
after. Limits are 512 MiB per file, 2 GiB overall, 16 MiB JSON, 2 MiB PE resources and
120 seconds. Unsupported platforms fail closed before selecting installed paths.

The command requires explicit SHA256 pins for an original reviewed V1 source-capture
manifest and ZIP archive. It hashes both, verifies the manifest/archive binding and
the exact selected ZIP members, then reads only the known versioned product JSON
metadata. It compares selected original Crew sources and package manifests when
they exist in that capture. A changed source is reported as `different_or_missing`.
Unavailable original bundle pins remain `not_available_in_pinned_source_capture`.
Registry/PE version agreement and current installed manifest claims are **not** original
build parity. The current manifests verify only their fixed allowlisted bundle hashes
and sizes; they cannot nominate additional paths. Archive/wheel-member claims and
historical smoke assertions are not independently verified or reexecuted.

Example from the V2 workspace (new exclusive output directory required):

```powershell
[Private development path omitted from this public documentation copy.]
[Private development path omitted from this public documentation copy.]
& $inventoryPython -I -B -S tools/inventory/windows_runtime_inventory.py `
[Private development path omitted from this public documentation copy.]
[Private development path omitted from this public documentation copy.]
  --source-manifest "$inventoryReference\manifest.json" `
  --source-manifest-sha256 cfc5a8e0d141fa94a7755804d549d8ec7acd3f067c5bbbe4c590d3d97986d4dd `
  --source-archive "$inventoryReference\source.zip" `
  --source-archive-sha256 d4c4f69a61392cdc75f73e9da036ee1aad198eacd208d9af10ac82bdf6d7efc4 `
[Private development path omitted from this public documentation copy.]
```

Only the new output directory is written, through an exclusive private stage and
atomic publication. Failures retain neither a partially published inventory nor a
replacement for an existing output. This is a partial M0 program inventory, not an
installation backup, complete installed-build qualification, full M0 completion,
database migration, or Stage 4 import.

The test suite uses real isolated Windows PE/package fixtures, hardlinks, native NTFS
junctions, pre-existing write handles, concurrent actual write/rename attempts, false
manifest/reference pins, registry drift and output conflicts. The successful fixture
flow has audit guards against installed directory enumeration, unselected installed
paths, SQLite opens and any subprocess launch. A separate CLI test exercises actual
registry refusal against an unmatched fixture root. Qualification should additionally
record a supervisor's positive native Popen wait/GetExitCodeProcess and empty owned
Job, actual installed inventory, and a real Linux fail-closed probe. It must not reuse
an earlier source's green result after edits.
