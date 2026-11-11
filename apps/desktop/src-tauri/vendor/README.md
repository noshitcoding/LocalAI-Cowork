# glib 0.18.5: local VariantStrIter source backport

This Rust crate is based on the complete crates.io `glib 0.18.5` archive
(267679 bytes, SHA256
`233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`).
Upstream VCS commit: `42b9caf98e03ded086362d9653ca58fe94dc8658`.
The original 121 ordinary archive members are retained; only
`glib/src/variant_iter.rs` has a production-code change. The published
Cargo.toml and Cargo.toml.orig are unchanged. Original `glib/LICENSE` (MIT)
and `glib/COPYRIGHT` are retained byte for byte. These notices and this
provenance file are selected as application resources under `licenses/glib/`.
This does not change the application's license or the licenses of native GTK
and GLib dependencies.

The production change is exactly the two-line fix from gtk-rs-core PR #1343:
https://github.com/gtk-rs/gtk-rs-core/commit/b5a4071e439bef2b5eea76c3aa25e5ae84839e34
The string out-pointer binding and its FFI argument are mutable. API,
dependency declarations, ABI, ownership, lifetime and iterator bounds stay
unchanged. Original `src/variant_iter.rs`: 9874 bytes / SHA256
`1fd02859333761c45321b32f28b24233446b97d0022a90d3a937ed162585b90e`.
Backported file: 9882 bytes / SHA256
`a0f5ee8acb8faa089bcdfbc9a57372609fce7654026ccef7d9a224d05a654ccc`.
Exact zero-context patch: 258 bytes / SHA256
`c94b65d2dd13f0c607589c12db308522dd3c936f97ed8762f688cc7ae83a3cac`.
The registry cache is not the patched source; the standalone Desktop manifest
selects this local crate through `[patch.crates-io]`.

`glib/tests/variant_str_iter_backport.rs` supplies four real-GLib regression
cases: empty/fused iteration, one Unicode borrow used after iterator drop
while the owning Variant stays alive, mixed forward/reverse iteration and
exact lengths, and out-of-range exhaustion. A separate standalone harness
selects this exact test source and the production crate as a normal path
dependency. It does not select glib's dev-dependencies or reproduce the full
Desktop/GTK feature union. Debug and test-profile opt-levels 2 and 3 are
separate execution checks; optimized execution matters for this aliasing
defect. Passing tests cannot prove that the original undefined behavior would
always manifest. Actual metadata/lock and execution evidence must be reviewed
separately; this source note asserts no resolver, test, package or installed
PASS. Native GLib development libraries are required, without a GUI, network,
provider or model.

Version remains 0.18.5. [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
reports >=0.20.0 as patched, so this source backport can remain within the
scanner's affected version range. No fake version, advisory ignore,
audit-exit relabeling or security-clean claim is used. A source hash manifest
binds the modified bytes; a version-only scanner finding is preserved rather
than silently dismissed.

Normal Cargo resolution must record the actual Desktop and independent
harness locks, selected path provenance, dependency versions and feature
sets. Earlier source results do not qualify this changed source/graph.
Source/material and license provenance, actual SBOMs, platform checks,
packages, installed acceptance, signing and update-feed qualification remain
separate requirements. See [dependency security](https://github.com/noshitcoding/LocalAI-Cowork/blob/main/docs/DEPENDENCY-SECURITY.md)
for the retained audit reports and the independent unresolved RSA finding.
