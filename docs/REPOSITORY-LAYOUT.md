# V2 and Enterprise repository layout

The existing Git history is retained. V2 replaces the former app at the root;
the separate Enterprise implementation is preserved under enterprise/.

| Path | Responsibility |
| --- | --- |
| apps/web, packages/ui, packages/client | Current V2 web UI and client |
| apps/desktop/src-tauri | Current V2 native desktop |
| crates, services | Current V2 core, daemon, agents and services |
| tools, tests, contracts | V2 tooling and source-bound tests/contracts |
| deploy/v2 | V2 deployment and image contexts |
| enterprise/Cargo.toml | Independent Enterprise workspace and lockfile |
| enterprise/server, enterprise/agents, enterprise/crates | Enterprise server, runner and agents |
| enterprise/app | Enterprise management web and retained server dependencies |
| enterprise/deploy, enterprise/scripts | Enterprise operator contexts and tools |
| site | Existing GitHub Pages website |

Run V2 commands at the root; run Enterprise commands from enterprise/ or select
enterprise/Cargo.toml explicitly. The workspaces reuse Rust package names and
must not be combined. Their versions identify different components.

Use enterprise/ as the Docker context for its existing Dockerfiles. Internal
Compose-relative paths retain their structure. See [self-hosting](PUBLIC-SELF-HOSTING.md).

Private profiles, evidence, runtime data, secrets, archives and caches are not
public source inputs. Fixed synthetic SQLite/JSONL verifier data, migration
reference sources and deliberate TLS test fixtures remain compiler/test inputs.
Never use the TLS fixture key in a deployment. Historical desktop source and
V1 downloads remain in Git history and their original release tags; legacy
desktop release workflows are not active V2 workflows.
