import { useCallback, useEffect, useMemo, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useNavigate } from 'react-router-dom'
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Check,
  ChevronRight,
  CircleDot,
  ExternalLink,
  FileCode2,
  FolderOpen,
  GitBranch,
  GitCommitHorizontal,
  GitPullRequest,
  GitMerge,
  LoaderCircle,
  LockKeyhole,
  MessageSquare,
  Plus,
  RefreshCw,
  Send,
  ShieldCheck,
  Sparkles,
  Unplug,
  UserRoundCheck,
  X,
} from 'lucide-react'
import { connectorLocator, deleteCredential, setCredential } from '../security/credentialVault'
import { useChatStore } from '../stores/chatStore'
import { useProjectStore } from '../stores/projectStore'
import { safeInvoke } from '../utils/safeInvoke'

type GitFileStatus = {
  path: string
  indexStatus: string
  worktreeStatus: string
  staged: boolean
  untracked: boolean
}

type RepositoryContext = {
  root: string
  owner: string | null
  repo: string | null
  remoteUrl: string | null
  webUrl: string | null
  branch: string
  defaultBranch: string
  ahead: number
  behind: number
  dirty: boolean
  files: GitFileStatus[]
}

type GitHubUser = {
  login: string
  avatarUrl: string
  htmlUrl: string
}

type ConnectionStatus = {
  authenticated: boolean
  profile: GitHubUser | null
  repository: RepositoryContext
}

type PullRequestSummary = {
  number: number
  title: string
  body: string
  state: string
  draft: boolean
  htmlUrl: string
  headBranch: string
  baseBranch: string
  user: GitHubUser
  createdAt: string
  updatedAt: string
  mergeable: boolean | null
  merged: boolean
}

type PullRequestFile = {
  filename: string
  status: string
  additions: number
  deletions: number
  changes: number
  patch: string
}

type PullRequestReview = {
  id: number
  user: GitHubUser
  body: string
  state: string
  submittedAt: string | null
  htmlUrl: string
}

type PullRequestComment = {
  id: number
  user: GitHubUser
  body: string
  createdAt: string
  updatedAt: string
  htmlUrl: string
}

type PullRequestDetail = PullRequestSummary & {
  files: PullRequestFile[]
  reviews: PullRequestReview[]
  comments: PullRequestComment[]
}

type GitMutationResult = {
  message: string
  repository: RepositoryContext
}

type GitDiffResponse = {
  diff: string
  truncated: boolean
}

type WorkbenchTab = 'changes' | 'pulls'

const GITHUB_CREDENTIAL = connectorLocator('github', 'api_key')
const REPOSITORY_STORAGE_KEY = 'localai-cowork:github:repository:v1'

function initialRepositoryPath(): string {
  const projectState = useProjectStore.getState()
  const activeProject = projectState.projects.find((project) => project.id === projectState.activeProjectId)
  const projectFolder = activeProject?.resources.find((resource) => resource.kind === 'folder' && resource.enabled)?.path
  return projectFolder || localStorage.getItem(REPOSITORY_STORAGE_KEY) || ''
}

function statusLabel(file: GitFileStatus): string {
  if (file.untracked) return 'U'
  if (file.staged && file.worktreeStatus !== ' ') return `${file.indexStatus}${file.worktreeStatus}`
  return file.staged ? file.indexStatus : file.worktreeStatus
}

function relativeTime(value: string): string {
  const timestamp = new Date(value).getTime()
  if (!Number.isFinite(timestamp)) return value
  const deltaMinutes = Math.round((timestamp - Date.now()) / 60_000)
  const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })
  if (Math.abs(deltaMinutes) < 60) return formatter.format(deltaMinutes, 'minute')
  const deltaHours = Math.round(deltaMinutes / 60)
  if (Math.abs(deltaHours) < 24) return formatter.format(deltaHours, 'hour')
  return formatter.format(Math.round(deltaHours / 24), 'day')
}

export default function GitHubView() {
  const navigate = useNavigate()
  const [cwd, setCwd] = useState(initialRepositoryPath)
  const [connection, setConnection] = useState<ConnectionStatus | null>(null)
  const [tokenDraft, setTokenDraft] = useState('')
  const [tab, setTab] = useState<WorkbenchTab>('changes')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [selectedPaths, setSelectedPaths] = useState<string[]>([])
  const [selectedFile, setSelectedFile] = useState<string | null>(null)
  const [diff, setDiff] = useState('')
  const [commitMessage, setCommitMessage] = useState('')
  const [branchDraft, setBranchDraft] = useState('')
  const [pulls, setPulls] = useState<PullRequestSummary[]>([])
  const [pullState, setPullState] = useState('open')
  const [selectedPull, setSelectedPull] = useState<PullRequestDetail | null>(null)
  const [showCreatePull, setShowCreatePull] = useState(false)
  const [pullTitle, setPullTitle] = useState('')
  const [pullBody, setPullBody] = useState('')
  const [pullBase, setPullBase] = useState('')
  const [pullDraft, setPullDraft] = useState(false)
  const [commentBody, setCommentBody] = useState('')
  const [reviewEvent, setReviewEvent] = useState('COMMENT')
  const [mergeMethod, setMergeMethod] = useState('squash')

  const repository = connection?.repository ?? null
  const stagedCount = useMemo(
    () => repository?.files.filter((file) => file.staged).length ?? 0,
    [repository],
  )

  const loadPulls = useCallback(async (path: string, state = pullState) => {
    const next = await safeInvoke<PullRequestSummary[]>('github_list_pull_requests', {
      request: { cwd: path, state },
    })
    setPulls(next)
    if (selectedPull && !next.some((pull) => pull.number === selectedPull.number)) {
      setSelectedPull(null)
    }
  }, [pullState, selectedPull])

  const loadRepository = useCallback(async (path = cwd, includePulls = true) => {
    const normalized = path.trim()
    if (!normalized) {
      setError('Choose a local Git repository.')
      return
    }
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const next = await safeInvoke<ConnectionStatus>('github_connection_status', {
        request: { cwd: normalized },
      })
      setConnection(next)
      setCwd(next.repository.root)
      setPullBase((current) => current || next.repository.defaultBranch)
      localStorage.setItem(REPOSITORY_STORAGE_KEY, next.repository.root)
      setSelectedPaths((current) => current.filter((path) => next.repository.files.some((file) => file.path === path)))
      if (includePulls && next.authenticated && next.repository.owner && next.repository.repo) {
        await loadPulls(next.repository.root)
      } else if (!next.authenticated) {
        setPulls([])
        setSelectedPull(null)
      }
    } catch (cause) {
      setConnection(null)
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }, [cwd, loadPulls])

  const runMutation = async (
    command: string,
    request: Record<string, unknown>,
    success?: string,
  ) => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const result = await safeInvoke<GitMutationResult>(command, { request: { cwd, ...request } })
      setConnection((current) => current ? { ...current, repository: result.repository } : current)
      setSelectedPaths([])
      setNotice(success || result.message || 'Done.')
      if (selectedFile && !result.repository.files.some((file) => file.path === selectedFile)) {
        setSelectedFile(null)
        setDiff('')
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const chooseRepository = async () => {
    const selected = await open({ directory: true, multiple: false, title: 'Choose Git repository' })
    if (typeof selected === 'string') {
      setCwd(selected)
      await loadRepository(selected)
    }
  }

  const saveToken = async () => {
    if (!tokenDraft.trim() || !cwd.trim()) return
    setBusy(true)
    setError(null)
    try {
      await setCredential(GITHUB_CREDENTIAL, tokenDraft.trim())
      setTokenDraft('')
      await loadRepository(cwd)
      setNotice('GitHub connected. The token is stored in the operating-system credential vault.')
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const disconnect = async () => {
    await deleteCredential(GITHUB_CREDENTIAL)
    setPulls([])
    setSelectedPull(null)
    if (cwd) await loadRepository(cwd, false)
  }

  const openDiff = async (file: GitFileStatus, staged = file.staged && file.worktreeStatus === ' ') => {
    setSelectedFile(file.path)
    setBusy(true)
    setError(null)
    try {
      const result = await safeInvoke<GitDiffResponse>('git_repository_diff', {
        request: { cwd, path: file.path, staged },
      })
      setDiff(result.diff || (file.untracked ? 'Untracked file. Stage it to view the complete patch.' : 'No diff in this area.'))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const openPull = async (number: number) => {
    setBusy(true)
    setError(null)
    try {
      const detail = await safeInvoke<PullRequestDetail>('github_get_pull_request', {
        request: { cwd, number },
      })
      setSelectedPull(detail)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const createPull = async () => {
    setBusy(true)
    setError(null)
    try {
      const created = await safeInvoke<PullRequestSummary>('github_create_pull_request', {
        request: {
          cwd,
          title: pullTitle,
          body: pullBody,
          baseBranch: pullBase,
          draft: pullDraft,
        },
      })
      setPullTitle('')
      setPullBody('')
      setPullDraft(false)
      setShowCreatePull(false)
      await loadPulls(cwd)
      await openPull(created.number)
      setNotice(`Pull request #${created.number} created.`)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      setBusy(false)
    }
  }

  const postComment = async () => {
    if (!selectedPull || !commentBody.trim()) return
    setBusy(true)
    setError(null)
    try {
      if (reviewEvent === 'ISSUE_COMMENT') {
        await safeInvoke('github_post_pull_request_comment', {
          request: { cwd, number: selectedPull.number, body: commentBody },
        })
      } else {
        await safeInvoke('github_submit_pull_request_review', {
          request: { cwd, number: selectedPull.number, body: commentBody, event: reviewEvent },
        })
      }
      setCommentBody('')
      await openPull(selectedPull.number)
      setNotice(reviewEvent === 'ISSUE_COMMENT' ? 'Comment posted.' : 'Review submitted.')
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      setBusy(false)
    }
  }

  const mergePull = async () => {
    if (!selectedPull) return
    if (!window.confirm(`Merge pull request #${selectedPull.number} using ${mergeMethod}?`)) return
    setBusy(true)
    setError(null)
    try {
      const result = await safeInvoke<{ merged: boolean; message: string }>('github_merge_pull_request', {
        request: { cwd, number: selectedPull.number, method: mergeMethod },
      })
      if (!result.merged) throw new Error(result.message)
      await loadPulls(cwd)
      await openPull(selectedPull.number)
      setNotice(result.message)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      setBusy(false)
    }
  }

  const prepareCoworkReview = () => {
    if (!selectedPull || !repository) return
    const patches = selectedPull.files
      .map((file) => `### ${file.filename}\n${file.patch || `[${file.status}, patch unavailable]`}`)
      .join('\n\n')
    const prompt = [
      `Review GitHub pull request #${selectedPull.number}: ${selectedPull.title}`,
      `Repository: ${repository.owner}/${repository.repo}`,
      `Branches: ${selectedPull.headBranch} -> ${selectedPull.baseBranch}`,
      `Local repository: ${repository.root}`,
      '',
      selectedPull.body,
      '',
      'Review the following patch for correctness, regressions, security issues, and missing tests.',
      'Return findings ordered by severity and cite file paths.',
      '',
      patches,
    ].join('\n')
    const threadId = useChatStore.getState().addThread(`Review PR #${selectedPull.number}`)
    useChatStore.getState().addMessage(threadId, {
      role: 'user',
      content: prompt,
      timestamp: Date.now(),
    })
    void useChatStore.getState().setActiveThread(threadId)
    navigate('/')
  }

  useEffect(() => {
    if (cwd) void loadRepository(cwd)
    // The initial repository is intentionally loaded once; later changes use explicit refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <main className="github-workbench">
      <header className="github-workbench-header">
        <div>
          <span className="github-workbench-kicker"><GitPullRequest size={14} /> GitHub workbench</span>
          <h1>Changes, pull requests, reviews</h1>
          <p>Local Git stays local. GitHub API credentials remain in the operating-system vault.</p>
        </div>
        <div className="github-auth">
          {connection?.authenticated && connection.profile ? (
            <>
              {connection.profile.avatarUrl
                ? <img src={connection.profile.avatarUrl} alt="" />
                : <UserRoundCheck size={24} />}
              <span><small>Connected as</small><strong>{connection.profile.login}</strong></span>
              <button type="button" onClick={disconnect} disabled={busy} title="Disconnect GitHub">
                <Unplug size={15} />
              </button>
            </>
          ) : (
            <span className="github-auth-status">
              <LockKeyhole size={15} />
              GitHub not connected
            </span>
          )}
        </div>
      </header>

      <section className="github-repository-bar">
        <GitBranch size={17} />
        <input
          value={cwd}
          onChange={(event) => setCwd(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') void loadRepository()
          }}
          placeholder="Choose a local Git repository"
          aria-label="Local Git repository"
        />
        <button type="button" onClick={chooseRepository} title="Choose repository" aria-label="Choose repository"><FolderOpen size={16} /></button>
        <button type="button" onClick={() => loadRepository()} disabled={!cwd || busy} title="Refresh">
          <RefreshCw size={16} className={busy ? 'spin' : ''} />
        </button>
      </section>

      {!connection?.authenticated && (
        <section className="github-connect-card">
          <div>
            <ShieldCheck size={24} />
            <span>
              <strong>Connect GitHub</strong>
              <small>Use a fine-grained personal access token with access only to the repositories and actions you need.</small>
            </span>
          </div>
          <form onSubmit={(event) => { event.preventDefault(); void saveToken() }}>
            <input
              type="password"
              autoComplete="off"
              value={tokenDraft}
              onChange={(event) => setTokenDraft(event.target.value)}
              placeholder="github_pat_…"
              aria-label="GitHub personal access token"
            />
            <button type="submit" className="ui-button ui-button--primary" disabled={!tokenDraft.trim() || !cwd.trim() || busy}>
              <LockKeyhole size={14} /> Save and verify
            </button>
          </form>
        </section>
      )}

      {error && <div className="github-message is-error" role="alert"><X size={15} /> {error}</div>}
      {notice && <div className="github-message is-success"><Check size={15} /> {notice}</div>}

      {repository ? (
        <>
          <section className="github-repository-summary">
            <div>
              <GitPullRequest size={21} />
              <span>
                <strong>{repository.owner && repository.repo ? `${repository.owner}/${repository.repo}` : 'Local Git repository'}</strong>
                <small>{repository.root}</small>
              </span>
            </div>
            <div className="github-branch-summary">
              <GitBranch size={14} /> {repository.branch}
              {repository.ahead > 0 && <span><ArrowUpFromLine size={12} /> {repository.ahead}</span>}
              {repository.behind > 0 && <span><ArrowDownToLine size={12} /> {repository.behind}</span>}
              {repository.webUrl && (
                <button type="button" onClick={() => void openUrl(repository.webUrl!)} title="Open on GitHub">
                  <ExternalLink size={14} />
                </button>
              )}
            </div>
          </section>

          <nav className="github-workbench-tabs" aria-label="GitHub workbench areas">
            <button type="button" className={tab === 'changes' ? 'active' : ''} onClick={() => setTab('changes')}>
              <GitCommitHorizontal size={15} /> Changes <span>{repository.files.length}</span>
            </button>
            <button
              type="button"
              className={tab === 'pulls' ? 'active' : ''}
              onClick={() => {
                setTab('pulls')
                if (connection?.authenticated) void loadPulls(cwd)
              }}
            >
              <GitMerge size={15} /> Pull requests <span>{pulls.length}</span>
            </button>
          </nav>

          {tab === 'changes' && (
            <div className="github-changes-layout">
              <section className="github-changes-panel">
                <div className="github-panel-heading">
                  <span><strong>Working tree</strong><small>{stagedCount} staged</small></span>
                  <div>
                    <button type="button" onClick={() => runMutation('git_repository_pull', {}, 'Repository updated.')} disabled={busy}>
                      <ArrowDownToLine size={14} /> Pull
                    </button>
                    <button type="button" onClick={() => runMutation('git_repository_push', {}, 'Branch pushed.')} disabled={busy}>
                      <ArrowUpFromLine size={14} /> Push
                    </button>
                  </div>
                </div>

                <div className="github-branch-create">
                  <input value={branchDraft} onChange={(event) => setBranchDraft(event.target.value)} placeholder="New branch name" />
                  <button
                    type="button"
                    onClick={() => {
                      void runMutation('git_repository_create_branch', { branch: branchDraft })
                      setBranchDraft('')
                    }}
                    disabled={!branchDraft.trim() || busy}
                  >
                    <Plus size={14} /> Create
                  </button>
                </div>

                <div className="github-file-list">
                  {repository.files.map((file) => (
                    <div key={file.path} className={selectedFile === file.path ? 'active' : ''}>
                      <input
                        type="checkbox"
                        checked={selectedPaths.includes(file.path)}
                        onChange={(event) => setSelectedPaths((current) => (
                          event.target.checked
                            ? [...new Set([...current, file.path])]
                            : current.filter((path) => path !== file.path)
                        ))}
                        aria-label={`Select ${file.path}`}
                      />
                      <button type="button" onClick={() => void openDiff(file)}>
                        <FileCode2 size={14} />
                        <span title={file.path}>{file.path}</span>
                        <code>{statusLabel(file)}</code>
                        <ChevronRight size={13} />
                      </button>
                    </div>
                  ))}
                  {repository.files.length === 0 && (
                    <div className="github-empty-state"><Check size={24} /><strong>Working tree clean</strong></div>
                  )}
                </div>

                {repository.files.length > 0 && (
                  <div className="github-stage-actions">
                    <button
                      type="button"
                      onClick={() => runMutation('git_repository_stage', {
                        paths: selectedPaths.length > 0 ? selectedPaths : repository.files.map((file) => file.path),
                      })}
                      disabled={busy}
                    >
                      Stage {selectedPaths.length > 0 ? selectedPaths.length : 'all'}
                    </button>
                    <button
                      type="button"
                      onClick={() => runMutation('git_repository_unstage', {
                        paths: selectedPaths.length > 0
                          ? selectedPaths
                          : repository.files.filter((file) => file.staged).map((file) => file.path),
                      })}
                      disabled={busy || stagedCount === 0}
                    >
                      Unstage
                    </button>
                  </div>
                )}

                <div className="github-commit-box">
                  <textarea
                    value={commitMessage}
                    onChange={(event) => setCommitMessage(event.target.value)}
                    placeholder="Commit message"
                    rows={3}
                  />
                  <button
                    type="button"
                    className="ui-button ui-button--primary"
                    onClick={() => {
                      void runMutation('git_repository_commit', { message: commitMessage })
                      setCommitMessage('')
                    }}
                    disabled={!commitMessage.trim() || stagedCount === 0 || busy}
                  >
                    <GitCommitHorizontal size={14} /> Commit {stagedCount} file{stagedCount === 1 ? '' : 's'}
                  </button>
                </div>
              </section>

              <section className="github-diff-panel">
                <div className="github-panel-heading">
                  <span><strong>{selectedFile || 'Diff preview'}</strong><small>Read-only</small></span>
                </div>
                <pre>{diff || 'Select a changed file to inspect its patch.'}</pre>
              </section>
            </div>
          )}

          {tab === 'pulls' && (
            connection?.authenticated ? (
              <div className="github-pulls-layout">
                <section className="github-pull-list-panel">
                  <div className="github-panel-heading">
                    <select value={pullState} onChange={(event) => {
                      setPullState(event.target.value)
                      void loadPulls(cwd, event.target.value)
                    }}>
                      <option value="open">Open</option>
                      <option value="closed">Closed</option>
                      <option value="all">All</option>
                    </select>
                    <button type="button" onClick={() => setShowCreatePull(true)}><Plus size={14} /> New pull request</button>
                  </div>
                  <div className="github-pull-list">
                    {pulls.map((pull) => (
                      <button
                        type="button"
                        key={pull.number}
                        className={selectedPull?.number === pull.number ? 'active' : ''}
                        onClick={() => void openPull(pull.number)}
                      >
                        <CircleDot size={15} />
                        <span>
                          <strong>{pull.title}</strong>
                          <small>#{pull.number} · {pull.headBranch} → {pull.baseBranch} · {relativeTime(pull.updatedAt)}</small>
                        </span>
                        {pull.draft && <em>Draft</em>}
                        <ChevronRight size={14} />
                      </button>
                    ))}
                    {pulls.length === 0 && <div className="github-empty-state"><GitMerge size={24} /><strong>No pull requests</strong></div>}
                  </div>
                </section>

                <section className="github-pull-detail">
                  {showCreatePull ? (
                    <div className="github-create-pull">
                      <div className="github-panel-heading">
                        <strong>New pull request</strong>
                        <button type="button" onClick={() => setShowCreatePull(false)}><X size={14} /></button>
                      </div>
                      <label>Title<input value={pullTitle} onChange={(event) => setPullTitle(event.target.value)} /></label>
                      <label>Base branch<input value={pullBase} onChange={(event) => setPullBase(event.target.value)} /></label>
                      <label>Description<textarea rows={8} value={pullBody} onChange={(event) => setPullBody(event.target.value)} /></label>
                      <label className="github-inline-check">
                        <input type="checkbox" checked={pullDraft} onChange={(event) => setPullDraft(event.target.checked)} />
                        Create as draft
                      </label>
                      <button type="button" className="ui-button ui-button--primary" onClick={createPull} disabled={!pullTitle.trim() || !pullBase.trim() || busy}>
                        <GitMerge size={14} /> Create pull request
                      </button>
                    </div>
                  ) : selectedPull ? (
                    <>
                      <div className="github-pull-title">
                        <span><CircleDot size={17} /> {selectedPull.state}{selectedPull.draft ? ' · draft' : ''}</span>
                        <h2>{selectedPull.title}</h2>
                        <p>#{selectedPull.number} by {selectedPull.user.login} · {selectedPull.headBranch} → {selectedPull.baseBranch}</p>
                        <div>
                          <button type="button" onClick={prepareCoworkReview}><Sparkles size={14} /> Prepare review in Cowork</button>
                          <button type="button" onClick={() => void openUrl(selectedPull.htmlUrl)}><ExternalLink size={14} /> GitHub</button>
                        </div>
                      </div>
                      {selectedPull.body && <div className="github-pull-body">{selectedPull.body}</div>}
                      <div className="github-pr-files">
                        <strong>{selectedPull.files.length} changed files</strong>
                        {selectedPull.files.map((file) => (
                          <details key={file.filename}>
                            <summary>
                              <span>{file.filename}</span>
                              <code className="additions">+{file.additions}</code>
                              <code className="deletions">-{file.deletions}</code>
                            </summary>
                            <pre>{file.patch || 'Patch unavailable for this file.'}</pre>
                          </details>
                        ))}
                      </div>
                      <div className="github-conversation">
                        {[...selectedPull.comments.map((comment) => ({
                          id: `comment-${comment.id}`,
                          user: comment.user,
                          body: comment.body,
                          state: 'COMMENT',
                          at: comment.createdAt,
                        })), ...selectedPull.reviews.map((review) => ({
                          id: `review-${review.id}`,
                          user: review.user,
                          body: review.body,
                          state: review.state,
                          at: review.submittedAt || '',
                        }))].map((entry) => (
                          <article key={entry.id}>
                            <strong>{entry.user.login}</strong><span>{entry.state} · {relativeTime(entry.at)}</span>
                            <p>{entry.body || '(No review body)'}</p>
                          </article>
                        ))}
                      </div>
                      {selectedPull.state === 'open' && (
                        <>
                          <div className="github-review-box">
                            <textarea value={commentBody} onChange={(event) => setCommentBody(event.target.value)} rows={5} placeholder="Leave a comment or review…" />
                            <select value={reviewEvent} onChange={(event) => setReviewEvent(event.target.value)}>
                              <option value="ISSUE_COMMENT">Comment</option>
                              <option value="COMMENT">Review comment</option>
                              <option value="APPROVE">Approve</option>
                              <option value="REQUEST_CHANGES">Request changes</option>
                            </select>
                            <button type="button" onClick={postComment} disabled={!commentBody.trim() || busy}><Send size={14} /> Submit</button>
                          </div>
                          <div className="github-merge-box">
                            <select value={mergeMethod} onChange={(event) => setMergeMethod(event.target.value)}>
                              <option value="squash">Squash and merge</option>
                              <option value="merge">Create merge commit</option>
                              <option value="rebase">Rebase and merge</option>
                            </select>
                            <button type="button" onClick={mergePull} disabled={busy}><GitMerge size={14} /> Merge pull request</button>
                          </div>
                        </>
                      )}
                    </>
                  ) : (
                    <div className="github-empty-state"><MessageSquare size={28} /><strong>Select a pull request</strong><span>Inspect files, discussion, and reviews.</span></div>
                  )}
                </section>
              </div>
            ) : (
              <div className="github-auth-required"><LockKeyhole size={30} /><strong>Connect GitHub to work with pull requests.</strong></div>
            )
          )}
        </>
      ) : (
        <div className="github-repository-empty">
          {busy ? <LoaderCircle size={30} className="spin" /> : <FolderOpen size={34} />}
          <strong>{busy ? 'Reading repository…' : 'Choose a Git repository'}</strong>
          <span>Local changes work without GitHub authentication.</span>
        </div>
      )}
    </main>
  )
}
