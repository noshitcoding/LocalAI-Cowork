# Team GitHub repository actions

The existing `github` team tool accepts both the existing remote API argument
vector and structured repository actions. Repository actions run real Git inside
the configured team tool image and use the encrypted GitHub credential selected
for that team. They never use a daemon owner's GitHub profile or token.

```json
{"operation":"clone","repository":"https://github.com/owner/project.git","directory":"project","depth":20}
```

Operations are `clone`, `checkout`, `fetch`, `pull`, `sync`, and `status`. Every
operation requires the exact HTTPS repository URL and a directory relative to
the current order workspace. `reference` accepts a branch, tag, or commit ID.
`checkout` requires it. `depth` is optional for clone, from 1 to 1000; omit it for
the complete history. Repository parent directories must already exist.

Clone creates a new directory and records the configured repository URL in a
credential-free receipt under `.git`. Subsequent operations require a repository
cloned by this tool for that same URL. Checkout of an explicit revision selects a
detached HEAD. Fetch downloads remote objects. Pull and sync fetch the selected
revision and merge only with `--ff-only`; they refuse an unclean working tree or
a divergent local history. On a detached HEAD, specify the branch explicitly for
pull or sync. Status and clone also support repositories with no commits.

In the team order screen, expand **GitHub-Repository im Team bearbeiten**, choose
the action and request it from the team agent. The regular run history shows the
exact typed action and container image for approval. No Git process starts before
that approval. The regular run cancellation, container ownership record and
tool journal cover the operation. A failed or uncertain operation is retained for
inspection and is never automatically repeated.

The adapter permits only HTTPS on the team's configured GitHub hostname, verifies
TLS and refuses redirects. It has a new private home and an empty template/hooks
directory. It replaces local Git configuration with a fixed credential-free
configuration; includes, URL rewrites, custom credential helpers, filters, proxy
settings and hooks are not retained. Linked worktrees, external object stores,
symlinked or hardlinked Git metadata and submodule recursion are refused. The
temporary token is supplied through an origin-bound askpass helper, never through
the repository URL, arguments or durable Git configuration. Command output is
bounded and credential forms are redacted. Git LFS filters and automatic
submodule checkout are not invoked by these repository actions.

The operator must rebuild the tool image from `deploy/v2/Dockerfile.tools` and
select its resulting immutable digest. The image includes
`team-github-repository.py`. Its network must permit the configured GitHub host.
The existing per-team GitHub configuration and selected run tool remain the
prerequisites; no new database schema or global operator credential is needed.
