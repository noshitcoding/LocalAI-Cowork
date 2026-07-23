# Developer Browser and GitHub Workbench

LocalAI Cowork includes two focused developer surfaces: a Chrome DevTools
Protocol (CDP) browser and a GitHub workbench. Both are available from the
top-level navigation.

## Developer Browser

The developer browser starts an installed Microsoft Edge, Google Chrome, or
Chromium executable in headless mode. It uses a dedicated profile below the
LocalAI Cowork application-data directory and never reuses the user's everyday
browser profile.

The browser surface supports:

- HTTP and HTTPS navigation, reload, back, and forward
- Screenshot-based pointer interaction, double-click, scrolling, typing, and
  common key presses
- DOM and visible-text snapshots
- Console capture
- Fetch, XHR, and resource request capture with status, duration, and transfer
  size where available
- Element inspection from screenshot coordinates
- Element-bound annotations stored locally in the app
- A generated Cowork prompt containing the current page and annotations
- A low-level CDP console for bounded commands in the Runtime, DOM, CSS,
  Network, Page, Performance, Accessibility, Emulation, Input, Log, and Overlay
  domains

The frontend communicates only with the Rust commands exposed by the Tauri
application. CDP is bound to `127.0.0.1` on a dynamically allocated port. The
browser process is stopped when the session ends; its reusable profile remains
dedicated to LocalAI Cowork.

### Current limitations

- Network and console data is collected by page instrumentation. It is a useful
  development view, not a byte-for-byte replacement for the full Chromium
  DevTools frontend.
- Snapshots return bounded recent console and network entries.
- Browser extensions and the user's normal authenticated browser session are
  intentionally unavailable.
- Only HTTP and HTTPS targets can be opened.
- Browser-wide, target-management, download, crash, and execution-termination
  CDP methods are not exposed by the low-level console.

## GitHub Workbench

Choose any local Git repository from the GitHub view. Local Git operations work
without a GitHub token:

- Inspect branch, upstream divergence, and working-tree status
- Inspect staged and unstaged diffs
- Stage and unstage selected files
- Create and switch to a branch
- Commit staged changes
- Fast-forward pull
- Push the current branch and establish its upstream

Repositories whose `origin` points to `github.com` can additionally use the
GitHub API:

- List open, closed, or all pull requests
- Inspect pull-request metadata, changed files, patches, comments, and reviews
- Create normal or draft pull requests from the current branch
- Post issue comments
- Submit comment, approval, or request-changes reviews
- Merge with merge-commit, squash, or rebase semantics
- Prepare a pull-request patch as a new Cowork review task

Local Git is invoked directly without a command shell. File paths and commit
messages are passed as individual process arguments, and file lists are
separated from Git options with `--`.

### GitHub authentication

Create a fine-grained personal access token on GitHub and grant access only to
the repositories and actions you intend to use. Paste it into **GitHub
Workbench → Connect GitHub**.

The token is stored through the existing operating-system credential vault. It
is not persisted in Zustand, local storage, application configuration, logs, or
Git command arguments. Disconnecting removes it from the vault.

Typical fine-grained permissions are:

- **Contents: Read and write** for pushing through the system Git credential
  helper and reading repository data
- **Pull requests: Read and write** for creating, reviewing, and merging pull
  requests
- **Issues: Read and write** for pull-request conversation comments

Git push and pull use the locally installed Git and its configured credential
helper. The API token stored in LocalAI Cowork is used only for GitHub REST API
requests.

### Safety and scope

- The workbench does not expose a one-click discard/reset action.
- Merge requires an explicit confirmation.
- Pull uses `--ff-only` and will not create an implicit merge commit.
- GitHub remotes with embedded credentials are redacted before errors are
  surfaced.
- API responses and diffs are bounded before they are returned to the UI.
- Pull-request file, review, and comment views currently load up to 100 entries
  per category; the pull-request list loads up to 50 entries.
