# Native owner tools and selected V1 content

The daemon explicitly configures absolute executable aliases in its private
`toolbox.json`. Native tools have that daemon owner's filesystem and network
rights. Managed order folders and process supervision are not a security sandbox;
Linux process groups cannot contain programs that deliberately escape them.
Untrusted execution needs the separate container runner configuration.

Each operator action first calls `preview`, then submits the same request with
its `approval_sha256`. The digest binds order, idempotency key, typed action,
resolved executable, arguments, working directory and limits. Runtime/model
dispatch additionally needs its current durable run/policy/approval admission;
a preview digest alone does not supply those permissions. Admission is journaled
before native launch. A lost physical result becomes `uncertain` and is never
automatically replayed. Timeout/cancel can occur after effects happened.

Supported action kinds are `terminal`, `browser`, `office` and `github`.
Terminal uses configured aliases, argv, relative order cwd, up to600 seconds
and1MiB capture per stream. The API supports polling and explicit cancellation.
Held native per-order file locks coordinate the local API and worker processes.
Shutdown stops and joins only locally admitted workers and selected-file captures.

Browser sessions use new private profiles bound to exactly one order. Configured
Chromium supports navigation, DOM and screenshot. Python Playwright additionally
supports click/fill; install it into the configured Python environment if needed.
Each call launches the selected URL in that persistent cookie/profile context;
DOM element state from an earlier browser process is not retained. Screenshots
can be downloaded through the order tool-file endpoint. No existing browser
profile is selected.

Office supports read/write text, read/create DOCX, read/create XLSX, read/create
PDF, read/create PPTX and LibreOffice conversion with a fresh private LO profile.
DOCX generation/reading and PPTX reading use Python's standard library; XLSX needs
`openpyxl`, PDF needs `pypdf`/`reportlab`, PPTX creation needs `python-pptx`.
Spreadsheet generated values remain text rather than executing formulas. Files
are relative to the managed order; the API permits explicit uploads/downloads.
The built-in document tools (`document_read`, `document_write`, `image_edit`) use
`execute_document` instead: SHA-checked input copies in a fresh private
`document-stages/<uuid>` stage, the pinned single-file adapters `document_engine.py`
and `image_engine.py` in their own `adapters/documents-<sha>` digest directory (so the
native preview hashes stay unchanged), LibreOffice launched directly with a macro-free
profile, and a validated receipt. LaTeX/Tectonic runs only inside OS confinement and
is disabled until that exists. Availability (`documents_available`, `latex_available`,
`office_convert_available`) comes from a cached `--probe`. See `docs/DOCUMENT-TOOLS.md`.
Github requires the daemon's explicitly selected private GitHub token file and
the installed `gh` CLI. It does not consult the owner's gh login profiles.

V1 import routes require both the owner bearer and native authority, reject browser
Origin, and read only the selected absolute JSON/SQLite path. Windows SQLite
capture reuses the fixed original exporter, deny-write/delete native fences,
WAL validation, a private snapshot and complete known Desktop0..28 profiles.
The original database is never SQLite-opened. Linux SQLite accepts only a declared
cold export with no WAL/SHM/journal, repeatable raw hashes and private immutable
SQLite inspection. That path does not claim a Windows native writer fence.

The selected JSON content format is:

```json
{"format":"localai-cowork-v1-content/1","projects":[{"id":"original-project-id","name":"Project","orders":[{"id":"original-order-id","title":"Order","draft":"User content","messages":[{"id":"original-message-id","role":"user","content":"Original text"}]}]}]}
```

Preview allocates fresh V2 project/order IDs and records original typed IDs/message
text in its receipt. Exact plan approval then commits all project/order/search
rows, visible imported-history records and the receipt in one database transaction; repeating that same commit
returns the same result. No legacy executable RunSpec, daemon actor/ACL, token,
approval, budget, schedule, outbox or resume/replay authority is adopted. Imported
chat text becomes an editable order draft. Original messages and embedded artifact
summaries/previews receive fresh content IDs with separate original IDs in the
visible imported history; their source paths remain text and are never opened.
JSON imports also accept explicit embedded text artifacts. No StoredRun is
fabricated for their foreign keys. This is the
personal user-content import basis, not a full historical M9/tenant transfer.
