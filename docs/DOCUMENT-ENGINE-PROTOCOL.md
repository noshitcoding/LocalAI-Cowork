# Document engine protocol

Current source contract, 24 October 2026, LocalAI Cowork V2 2.0.0-alpha.7.
The adapters are `crates/toolbox/src/document_engine.py` and `image_engine.py`.
The authority for accepted model arguments is `crates/domain/src/documents.rs`;
format specifications are returned by `document_read` with `operation: "help"`.

## Admission and human actions

Chat tools and the document workbench both execute `document_read`, `document_write`
and `image_edit` through the runtime, policy, broker and Generate journal.
The workbench does not need a provider or a native-owner capability.

Human actions use `DocumentActionRequest`, version **2**, with an order revision,
idempotency key, access mode and tool call. `POST /orders/{order}/document-actions/prepare`
validates the tool and binds source SHA-256 values, folder identities, output, executable
selection and adapter revision. Preparation is durable before a response is returned.
The fingerprint covers the immutable run specification. `GET .../by-key/{key}` recovers
a lost prepare acknowledgement without resubmitting document content.

`POST .../{preparation}/commit` accepts no replacement tool or specification. It checks
the reviewed runtime, sources and destination, then atomically creates a model-free run.
Repeating an accepted commit returns the original run, including after publication.
The selected access mode governs run approvals. Reads narrow folder grants to read only;
mutations grant write access only to the actual output folder. Outputs never overwrite.

Drafts are independent of runs. `PUT /orders/{order}/document-drafts` atomically stores
the head, immutable snapshot and idempotent response. A stale base revision retains a
conflict copy without replacing the head. Scope is `(order, draft id)`; a draft is at most
512 KiB of JSON. The snapshot route returns the latest 50 saved or conflict versions.
The browser action journal stores the key and prepare/commit phase, not document text.

## Private stage

Rust embeds both adapters under a digest-bound directory and launches Python with
`-I -X utf8`. One executable selection is retained across adapter/conversion phases.
Every invocation owns a fresh stage:

```
request.json
in/…                  SHA-verified input copies
work/                 intermediate conversion and compilation files
work/previews/        retained PNG previews
out/                  candidate files for publication
home/                 private process home
tmp/                  temporary files
```

Inputs are limited to 16 files of 32 MiB each; image overlays are limited to 8. TeX
assets share the main file's granted folder and lie beneath its directory. Relative
paths must be portable and cannot escape the stage. Image pipelines contain 1–50 steps.
There are at most two concurrent engine jobs, with a maximum overall duration of 300 s.

## Request and receipt

`request.json` uses `cowork-document-request/1` and contains `tool`, `operation`,
`phase`, `inputs`, `output`, `spec`, `options` and `limits`. Each input contains its
stage `name`, user-visible `path`, `sha256` and byte count. `output` is absent for reads.
Only Rust supplies stage paths and multi-phase options.

The adapter writes exactly one JSON receipt to stdout. `cowork-document-receipt/1`
with `ok: true` requires exit 0; `ok: false` requires exit 2 and an error code/message.
Other exits, malformed receipts or oversized stdout are engine failures. A receipt is
at most 1 MiB. The model result is at most 32 KiB, with explicit pagination for content.

Success receipts contain `engine_version`, `result`, `outputs`, `previews`,
`diagnostics` and optional `qa`. Output records contain relative stage names, SHA-256,
byte counts and media types. Rust verifies actual files before publishing at most
64 files/64 MiB through the broker. Only files below `out/` may be published. PDF split
is published as one new directory. Preview records reference PNGs below
`work/previews/`, at most 4 images, 1600 px edge and 2 MiB each under default limits.
Run preview endpoints expose only hashes actually referenced by that run.

## Multi-phase operations

- Office preview: Rust converts a staged Office input to PDF with a new LibreOffice
  profile, then invokes `render_pdf` with the staged output reference.
- Office conversion: Rust invokes LibreOffice and then `office_finalize`, which
  validates the produced PDF/OOXML before copying to the publish directory.
- XLSX recalculation: explicit `convert`, XLSX input, new XLSX output and
  `spec.recalculate: true`. `recalculate_prepare` removes formula caches only from a
  private copy, preserves formulas, requests automatic full calculation and records
  source/prepared hashes. External relationships, network formulas and unsupported
  array/data-table formulas are refused before LibreOffice starts. Finalization requires
  the bound copy and unchanged formula count. `qa.calculation` reports `formula_cells`, `missing_values`,
  `error_values`, engine and status. Existing source caches are `unverified`;
  creating/editing with openpyxl does not count as calculation. A LibreOffice result
  with missing or error values is reported as `incomplete` or `errors`;
  `input_caches_cleared` distinguishes this path from a plain format conversion.
- PDF creation: `latex_prepare` → isolated Tectonic → `latex_finalize` if LaTeX is
  ready, otherwise the ReportLab layout. Custom TeX always needs qualified isolation.
- TeX compilation: preserved relative input tree → isolated Tectonic with
  `--untrusted --only-cached` → parsed diagnostics, QA and optional PNG previews.

LibreOffice has macros disabled in its private profile and a 90 s deadline. Tectonic
uses Windows AppContainer or Linux Landlock/seccomp. Python and LibreOffice currently
run under the user's OS identity; their stage/audit restrictions are not an OS sandbox.
Each Office invocation owns a separate short profile at `<toolbox>/.lo/<UUID>`;
the UUID uses a 22-character URL-safe Base64 encoding without losing random bits.
It is removed when the invocation ends. Windows rejects profile paths longer than
128 UTF-16 code units, reserving space for LibreOffice's internal files. A longer
profile nested inside the document stage was observed to exit successfully without
producing a Calc result. Readiness reports `data_path_too_long` separately from a
failed conversion probe. Startup cleans only ordinary UUID-named directories in
this private profile area, accepting both the compact and previous hex formats.
Converter failures include bounded, path-redacted diagnostics.

## Spreadsheet preservation

Row/column insertions and deletions update supported A1 references across sheets,
absolute references, whole-row/column ranges and workbook/local defined names. Sheet
rename/deletion updates those references too; deleted cells/sheets become `#REF!`.
String literals and references belonging to other sheets are preserved. Row/column
dimensions move with their cells. Sparse shifts cannot expand beyond one million cells.
This explicit logic is necessary because [openpyxl does not update these dependencies](https://openpyxl.readthedocs.io/en/stable/editing_worksheets.html).

Structural changes refuse external/3D/structured/spill references and objects whose
dependencies are not implemented: tables, charts, validation and conditional formats;
row/column shifts additionally refuse anchored drawings and affected merged/frozen/filter/
print ranges. This is a supported subset, not complete Excel reference semantics.
Ordinary cell edits preserve basic chart/image parts and verify their presence, drawing
object counts and image bytes after serialization. Unsupported package parts (including
pivots, ActiveX/OLE, signatures and unknown extension content) are rejected. Template
creation uses the same preservation guards. These checks do not prove visual fidelity.

## Image editing drafts

The workbench exposes all 19 existing image pipeline operations through parameter
controls, with at most 50 ordered steps, reorder/remove and bounded undo/redo. A version-1
image draft stores the original folder/path/SHA, steps, overlay paths, output and encoding
options inside the server-side document draft. Loading validates its shape; changing the
opened source version requires an explicit new image draft. Review binds overlay hashes
through the ordinary document preparation. EXIF retention is explicit and off by default.
The source preview and the actual post-execution result are distinct; there is no
pixel-exact interactive canvas preview yet.

## Availability and qualification

### LaTeX project drafts

The workbench captures a version-1 LaTeX manifest in its server-side document draft:
folder, main file, PDF output and an explicit list of paths, SHA-256 values and sizes.
The existing bound is 16 files, each at most 32 MiB. Capture requires every file to
exist in the same workspace; companions must lie beneath the main file's directory.
Main-file selection preserves this constraint. Compilation sends the main file first
and binds every captured hash through the ordinary document preparation.

Project source navigation downloads only an exact manifest member against its captured
hash, at most 512 KiB of UTF-8 text. Recognized diagnostic locations become source
buttons and select the reported line. Host paths, traversal and unknown package files
cannot trigger arbitrary downloads. Changing a source requires an explicit recapture;
loading a draft does not silently accept a newer file. Larger projects, direct source
publication and the broader bibliography/package corpus remain open.
Diagnostic buttons also require the loaded manifest to match the reviewed compilation's
main file, folder and every input hash, so loading another draft cannot attach an old
error to a newer source. Workspace lookups wait at most one second for initialization
lock contention from simultaneous UI polling. This retries only the workspace check;
tool execution and publication are never replayed by this wait.
The log parser resolves Tectonic's extensionless `\input` entries only against the
staged source list and prefers the most specific path when basenames overlap.

### Runtime checks

`GET /toolbox` distinguishes `probing`, `missing_runtime`, `probe_failed`,
`cache_incomplete`, `isolation_unavailable` and `ready` where applicable. Document
availability requires a successful adapter probe. TeX requires a matching warm marker
and an actual isolated binary probe. Office conversion requires a successful private
text-to-PDF conversion, not merely an executable path.
The Office probe has the same 90-second conversion bound as actual Office jobs;
first creation of a private profile can exceed 30 seconds on Windows.

`tools/toolbox/qualify_documents.py --install-tex` provisions a private pinned TeX
runtime. Alternatively it copies explicitly configured binary/cache paths into its own
temporary directory. The required mode fails on skipped prerequisites. It runs installer,
engine, domain/policy, broker, runtime and OS-confinement tests. CI defines Windows and
Linux qualification jobs plus `tests/system/documents.py` for real HTTP/restart tests.
Defining a CI job does not constitute an executed target qualification.

The separate Office CI job provisions only the SHA-pinned private LibreOffice using
`runtime_install.py --data-dir .local/office-ci --libreoffice-only`, then requires
`tests/system/documents.py --toolbox-config .local/office-ci/toolbox.json` to pass real
recalculation and DOCX/PPTX/XLSX-to-PDF cases. A missing or failed Office probe fails this
qualification. No global MSI installation action is invoked.
On Windows the private setup prefers `soffice.com`, the console entry point described
in the [LibreOffice command-line documentation](https://help.libreoffice.org/latest/en-GB/text/shared/guide/start_parameters.html?DbPAR=BASIC&System=WIN).
An explicitly configured existing executable is preserved. Qualification uses the same
small environment and private home/temp paths as document jobs.
`qualify_office.py --toolbox-config ...` additionally checks real calculated values
after row insertion and renaming a sheet containing an apostrophe, including defined
names and a string literal that resembles a reference. This is a generated corpus;
it does not replace Microsoft Office interoperability or rendering comparisons.

`tests/system/documents.py --browser` requires the current web build and Playwright
Chromium. In a disposable daemon it checks actual decoded source/result images,
resize/rotation, undo/redo, reordering, server draft persistence across reload, review,
approval, the generated PNG dimensions and the narrow viewport. The normal HTTP
assertions also verify CSP directives for image/PDF blobs while preserving restricted
script/object policies. CI defines a separate Windows/Linux browser job; a missing
browser or web build fails it instead of skipping the flow.
The browser flow also captures a LaTeX manifest, switches its main file, restores the
manifest after reload and refuses to substitute changed source content.

`tests/system/documents.py --latex` requires explicit `COWORK_TECTONIC` and
`COWORK_TECTONIC_CACHE` paths. It copies both into the disposable runtime before any
confinement ACL changes, requires actual isolation readiness and compiles a project
with a companion source, image and cross-reference. It checks restart, changed hashes,
source diagnostics without publication and corrected-source compilation. The mandatory
`qualify_documents.py` runner includes this real HTTP qualification.
