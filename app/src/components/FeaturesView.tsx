import { useMemo, useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { Blocks, Brain, Command, PlugZap, Search, Sparkles } from 'lucide-react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { tr } from '../i18n'
import { useCommandRegistry } from '../stores/commandRegistryStore'
import McpView from './McpView'
import MemoryPanel from './MemoryPanel'
import SkillPanel from './SkillPanel'

type WorkbenchTab = 'mcp' | 'knowledge' | 'skills' | 'commands'

const TABS: Array<{ id: WorkbenchTab; label: string; description: string; icon: typeof PlugZap }> = [
  { id: 'mcp', label: 'MCP Server', description: 'Connect external tools and runtimes', icon: PlugZap },
  { id: 'knowledge', label: 'Knowledge base', description: 'Curate reusable workspace context', icon: Brain },
  { id: 'skills', label: 'Skills', description: 'Shape repeatable expert workflows', icon: Sparkles },
  { id: 'commands', label: 'Slash commands', description: 'Launch actions without leaving chat', icon: Command },
]

function isWorkbenchTab(value: string | null): value is WorkbenchTab {
  return TABS.some((tab) => tab.id === value)
}

export default function FeaturesView() {
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const requestedTab = searchParams.get('tab')
  const activeTab: WorkbenchTab = isWorkbenchTab(requestedTab) ? requestedTab : 'mcp'
  const commands = useCommandRegistry((state) => state.commands)
  const [commandQuery, setCommandQuery] = useState('')
  const tabRefs = useRef<Partial<Record<WorkbenchTab, HTMLButtonElement | null>>>({})

  const filteredCommands = useMemo(() => {
    const query = commandQuery.trim().toLowerCase()
    if (!query) return commands
    return commands.filter((command) => (
      `${command.command} ${command.label} ${command.description} ${command.category}`.toLowerCase().includes(query)
    ))
  }, [commandQuery, commands])

  const selectTab = (tab: WorkbenchTab) => {
    const next = new URLSearchParams(searchParams)
    next.set('tab', tab)
    setSearchParams(next, { replace: true })
  }

  const openCommandInChat = (command: string) => {
    navigate(`/?slash=${encodeURIComponent(command)}`)
  }

  const handleTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const lastIndex = TABS.length - 1
    let nextIndex = index
    if (event.key === 'ArrowRight') nextIndex = index === lastIndex ? 0 : index + 1
    else if (event.key === 'ArrowLeft') nextIndex = index === 0 ? lastIndex : index - 1
    else if (event.key === 'Home') nextIndex = 0
    else if (event.key === 'End') nextIndex = lastIndex
    else return

    event.preventDefault()
    const nextTab = TABS[nextIndex]
    selectTab(nextTab.id)
    window.requestAnimationFrame(() => tabRefs.current[nextTab.id]?.focus())
  }

  return (
    <main className="feature-workbench">
      <header className="feature-workbench-header">
        <div className="feature-workbench-heading">
          <span className="feature-workbench-mark"><Blocks size={22} aria-hidden="true" /></span>
          <div>
            <span className="feature-workbench-kicker">{tr('Capability center')}</span>
            <h1>{tr('Tools and knowledge')}</h1>
            <p>{tr('Connect MCP tools, maintain reusable knowledge, manage skills, and discover commands.')}</p>
          </div>
        </div>
        <div className="feature-workbench-metrics" aria-label={tr('Capability overview')}>
          <span><strong>{TABS.length}</strong>{tr('workbenches')}</span>
          <span><strong>{commands.length}</strong>{tr('commands ready')}</span>
        </div>
      </header>

      <div className="feature-workbench-tabs" role="tablist" aria-label={tr('Tools and knowledge')}>
        {TABS.map((tab, index) => {
          const Icon = tab.icon
          return (
            <button
              type="button"
              key={tab.id}
              id={`feature-tab-${tab.id}`}
              ref={(element) => { tabRefs.current[tab.id] = element }}
              role="tab"
              aria-label={tr(tab.label)}
              aria-selected={activeTab === tab.id}
              aria-controls="feature-workbench-panel"
              tabIndex={activeTab === tab.id ? 0 : -1}
              className={activeTab === tab.id ? 'active' : ''}
              onClick={() => selectTab(tab.id)}
              onKeyDown={(event) => handleTabKeyDown(event, index)}
            >
              <span className="feature-workbench-tab-icon"><Icon size={17} aria-hidden="true" /></span>
              <span className="feature-workbench-tab-copy">
                <strong>{tr(tab.label)}</strong>
                <small>{tr(tab.description)}</small>
              </span>
            </button>
          )
        })}
      </div>

      <section
        id="feature-workbench-panel"
        className="feature-workbench-body"
        role="tabpanel"
        aria-labelledby={`feature-tab-${activeTab}`}
      >
        {activeTab === 'mcp' && <McpView />}
        {activeTab === 'knowledge' && <MemoryPanel />}
        {activeTab === 'skills' && <SkillPanel />}
        {activeTab === 'commands' && (
          <div className="command-workbench">
            <div className="command-workbench-toolbar">
              <label className="command-workbench-search">
                <Search size={17} aria-hidden="true" />
                <input
                  type="search"
                  value={commandQuery}
                  onChange={(event) => setCommandQuery(event.target.value)}
                  placeholder={tr('Search slash commands...')}
                  aria-label={tr('Search slash commands...')}
                />
              </label>
              <span className="command-workbench-count"><strong>{filteredCommands.length}</strong>{tr('commands')}</span>
            </div>
            <div className="command-workbench-list">
              {filteredCommands.map((command) => (
                <button type="button" key={command.id} onClick={() => openCommandInChat(command.command)}>
                  <code>{command.command}</code>
                  <span className="command-workbench-copy">
                    <strong>{tr(command.label)}</strong>
                    <small>{tr(command.description)}</small>
                  </span>
                  <span className="command-workbench-category">{tr(command.category)}</span>
                </button>
              ))}
              {filteredCommands.length === 0 && (
                <div className="command-workbench-empty">
                  <Search size={22} aria-hidden="true" />
                  <strong>{tr('No commands match your search')}</strong>
                  <span>{tr('Try another command name or category.')}</span>
                </div>
              )}
            </div>
          </div>
        )}
      </section>
    </main>
  )
}
