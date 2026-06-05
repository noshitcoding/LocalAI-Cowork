import { useEffect, useState } from 'react'
import { useMemoryStore, type MemoryEntry } from '../stores/memoryStore'
import { tr } from '../i18n'

function randomId() {
  return `mem-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export default function MemoryPanel() {
  const {
    entries, searchResults, profileEntries, providers, hints, lastSnapshot,
    loading, error,
    loadEntries, searchEntries, upsertEntry, deleteEntry,
    compactEntries, createSnapshot, loadHints,
    loadProfile, upsertProfile, deleteProfile,
    loadProviders, upsertProvider, deleteProvider,
  } = useMemoryStore()

  const [tab, setTab] = useState<'entries' | 'profile' | 'providers' | 'hints'>('entries')
  const [searchQuery, setSearchQuery] = useState('')
  const [filterScope, setFilterScope] = useState('')
  const [showAdd, setShowAdd] = useState(false)
  const [newEntry, setNewEntry] = useState({ scope: 'agent', category: 'general', key: '', content: '' })
  const [compactScope, setCompactScope] = useState('agent')
  const [compactMinConf, setCompactMinConf] = useState(0.3)
  const [compactResult, setCompactResult] = useState<string | null>(null)
  const [profileKey, setProfileKey] = useState('')
  const [profileValue, setProfileValue] = useState('')
  const [providerForm, setProviderForm] = useState({ name: '', type: 'mem0', config: '{}' })

  useEffect(() => {
    loadEntries(filterScope || undefined, undefined, 200)
  }, [filterScope, loadEntries])

  useEffect(() => {
    if (tab === 'profile') loadProfile()
    if (tab === 'providers') loadProviders()
    if (tab === 'hints') loadHints()
  }, [tab, loadProfile, loadProviders, loadHints])

  const handleSearch = () => {
    if (searchQuery.trim()) searchEntries(searchQuery.trim())
  }

  const handleAdd = async () => {
    if (!newEntry.key.trim() || !newEntry.content.trim()) return
    await upsertEntry({ id: randomId(), ...newEntry })
    setShowAdd(false)
    setNewEntry({ scope: 'agent', category: 'general', key: '', content: '' })
    loadEntries(filterScope || undefined)
  }

  const handleCompact = async () => {
    const r = await compactEntries(compactScope, compactMinConf)
    setCompactResult(`${r.removed} entfernt, ${r.remaining} verbleibend`)
    loadEntries(filterScope || undefined)
  }

  const handleSnapshot = async () => {
    await createSnapshot()
  }

  const displayEntries = searchQuery.trim() ? searchResults : entries

  return (
    <div className="panel">
      <h2>{tr("🧠 Agent memory")}</h2>

      <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
        {(['entries', 'profile', 'providers', 'hints'] as const).map((t) => (
          <button type="button" key={t} className={`btn-sm${tab === t ? ' active' : ''}`}
            onClick={() => setTab(t)}>
            {t === 'entries' ? tr('Entries') : t === 'profile' ? tr('Profile') : t === 'providers' ? tr('Provider') : tr('Hints')}
          </button>
        ))}
      </div>

      {error && <p style={{ color: 'var(--danger)', fontSize: 12 }}>{error}</p>}

      {tab === 'entries' && (
        <>
          {/* Search bar */}
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            <input
              type="text"
              placeholder={tr("Search memory...")}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
              style={{ flex: 1, padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', background: 'var(--bg-secondary)', fontSize: 13 }}
            />
            <select value={filterScope} onChange={(e) => setFilterScope(e.target.value)}
              style={{ padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13 }}>
              <option value="">{tr("All Scopes")}</option>
              <option value="agent">{tr("Agent")}</option>
              <option value="user">{tr("User")}</option>
              <option value="session">{tr("Session")}</option>
              <option value="shared">{tr("Shared")}</option>
            </select>
            <button type="button" className="btn-sm" onClick={() => setShowAdd(!showAdd)}>{tr("New")}</button>
          </div>

          {/* Add form */}
          {showAdd && (
            <div className="card" style={{ marginBottom: 12 }}>
              <div className="grid" style={{ gridTemplateColumns: '1fr 1fr', marginBottom: 8 }}>
                <label>{tr("Scope")}<select value={newEntry.scope} onChange={(e) => setNewEntry({ ...newEntry, scope: e.target.value })}>
                    <option value="agent">{tr("Agent")}</option>
                    <option value="user">{tr("User")}</option>
                    <option value="session">{tr("Session")}</option>
                    <option value="shared">{tr("Shared")}</option>
                  </select>
                </label>
                <label>{tr("Category")}<input type="text" value={newEntry.category} onChange={(e) => setNewEntry({ ...newEntry, category: e.target.value })} />
                </label>
              </div>
              <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13, marginBottom: 8 }}>{tr("Key")}<input type="text" value={newEntry.key} onChange={(e) => setNewEntry({ ...newEntry, key: e.target.value })}
                  style={{ padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13 }} />
              </label>
              <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13, marginBottom: 8 }}>{tr("Content")}<textarea value={newEntry.content} onChange={(e) => setNewEntry({ ...newEntry, content: e.target.value })} rows={3}
                  style={{ padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13, resize: 'vertical' }} />
              </label>
              <button type="button" className="btn-sm" onClick={handleAdd}>{tr("Save")}</button>
            </div>
          )}

          {/* Compact & Snapshot */}
          <div style={{ display: 'flex', gap: 8, marginBottom: 12, alignItems: 'center', flexWrap: 'wrap' }}>
            <select value={compactScope} onChange={(e) => setCompactScope(e.target.value)}
              style={{ padding: '4px 8px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 12 }}>
              <option value="agent">{tr("Agent")}</option>
              <option value="user">{tr("User")}</option>
              <option value="session">{tr("Session")}</option>
            </select>
            <input type="number" value={compactMinConf} onChange={(e) => setCompactMinConf(Number(e.target.value))} min={0} max={1} step={0.1}
              style={{ width: 60, padding: '4px 8px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 12 }} />
            <button type="button" className="btn-sm" onClick={handleCompact}>{tr("Compact")}</button>
            <button type="button" className="btn-sm" onClick={handleSnapshot}>{tr("📸 Snapshot")}</button>
            {compactResult && <span style={{ fontSize: 11, color: 'var(--success)' }}>{compactResult}</span>}
          </div>

          {/* Entries list */}
          {loading ? (
            <p className="panel-empty">{tr("Loading...")}</p>
          ) : displayEntries.length === 0 ? (
            <p className="panel-empty">{tr("No entries found")}</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {displayEntries.map((entry: MemoryEntry) => (
                <div key={entry.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 8 }}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 2 }}>
                      {entry.scope} / {entry.category}{tr("&middot; Conf:")}{entry.confidence.toFixed(2)}{tr("&middot; Zugriffe:")}{entry.access_count}
                    </div>
                    <div style={{ fontWeight: 600, fontSize: 13, marginBottom: 2 }}>{entry.key}</div>
                    <div style={{ fontSize: 12, color: 'var(--text-secondary)', whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                      {entry.content.length > 200 ? `${entry.content.slice(0, 200)}...` : entry.content}
                    </div>
                  </div>
                  <button type="button" onClick={() => deleteEntry(entry.id)} title={tr("Delete")}
                    style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--danger)', fontSize: 14 }}>{tr("×")}</button>
                </div>
              ))}
            </div>
          )}

          {lastSnapshot && (
            <div className="card" style={{ marginTop: 12, fontSize: 12 }}>
              <strong>{tr("Last snapshot:")}</strong> {lastSnapshot.total_entries}{tr("entries,")}{lastSnapshot.total_profile_keys}{tr("profile keys")}<br />
              <span style={{ color: 'var(--text-muted)' }}>{tr("Created:")}{lastSnapshot.timestamp}</span>
            </div>
          )}
        </>
      )}

      {tab === 'profile' && (
        <>
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            <input type="text" placeholder={tr("Key")} value={profileKey} onChange={(e) => setProfileKey(e.target.value)}
              style={{ flex: 1, padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13 }} />
            <input type="text" placeholder={tr("Wert")} value={profileValue} onChange={(e) => setProfileValue(e.target.value)}
              style={{ flex: 2, padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13 }} />
            <button type="button" className="btn-sm" onClick={async () => {
              if (profileKey.trim() && profileValue.trim()) {
                await upsertProfile(profileKey.trim(), profileValue.trim())
                setProfileKey('')
                setProfileValue('')
                loadProfile()
              }
            }}>+</button>
          </div>
          {profileEntries.length === 0 ? (
            <p className="panel-empty">{tr("No profile created")}</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {profileEntries.map((p) => (
                <div key={p.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <div>
                    <strong style={{ fontSize: 13 }}>{p.key}:</strong>{' '}
                    <span style={{ fontSize: 12 }}>{p.value}</span>
                    <span style={{ fontSize: 11, color: 'var(--text-muted)', marginLeft: 8 }}>({p.source}, {p.confidence.toFixed(1)})</span>
                  </div>
                  <button type="button" onClick={() => { deleteProfile(p.key); loadProfile() }} title={tr("Delete")}
                    style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--danger)', fontSize: 14 }}>{tr("×")}</button>
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {tab === 'providers' && (
        <>
          <div className="card" style={{ marginBottom: 12 }}>
            <div className="grid" style={{ gridTemplateColumns: '1fr 1fr 2fr' }}>
              <label>{tr("Name")}<input type="text" value={providerForm.name} onChange={(e) => setProviderForm({ ...providerForm, name: e.target.value })} />
              </label>
              <label>{tr("Type")}<select value={providerForm.type} onChange={(e) => setProviderForm({ ...providerForm, type: e.target.value })}>
                  <option value="mem0">{tr("Mem0")}</option>
                  <option value="honcho">{tr("Honcho")}</option>
                  <option value="supermemory">{tr("Supermemory")}</option>
                  <option value="custom">{tr("Custom")}</option>
                </select>
              </label>
              <label>{tr("Config (JSON)")}<input type="text" value={providerForm.config} onChange={(e) => setProviderForm({ ...providerForm, config: e.target.value })} />
              </label>
            </div>
            <button type="button" className="btn-sm" style={{ marginTop: 8 }} onClick={async () => {
              if (providerForm.name.trim()) {
                const id = `prov-${Date.now()}`
                await upsertProvider({ id, name: providerForm.name, provider_type: providerForm.type, config_json: providerForm.config })
                setProviderForm({ name: '', type: 'mem0', config: '{}' })
                loadProviders()
              }
            }}>{tr("Provider add")}</button>
          </div>
          {providers.length === 0 ? (
            <p className="panel-empty">{tr("No Provider configured")}</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {providers.map((p) => (
                <div key={p.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <div>
                    <strong>{p.name}</strong>{' '}
                    <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>({p.provider_type})</span>
                    {!p.enabled && <span style={{ fontSize: 11, color: 'var(--warning)', marginLeft: 6 }}>{tr("disabled")}</span>}
                  </div>
                  <button type="button" onClick={() => { deleteProvider(p.id); loadProviders() }} title={tr("Delete")}
                    style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--danger)', fontSize: 14 }}>{tr("×")}</button>
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {tab === 'hints' && (
        <>
          {hints.length === 0 ? (
            <p className="panel-empty">{tr("No notes available")}</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {hints.map((h, i) => (
                <div key={i} className="card">
                  <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 2 }}>
                    {h.scope}{tr("&middot; Relevanz:")}{h.relevance}
                  </div>
                  <div style={{ fontWeight: 600, fontSize: 13 }}>{h.key}</div>
                  <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{h.content}</div>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
