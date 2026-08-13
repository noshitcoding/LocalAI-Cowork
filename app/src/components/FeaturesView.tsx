import { useMemo, useState } from 'react'
import { Search } from 'lucide-react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { tr } from '../i18n'
import { useCommandRegistry } from '../stores/commandRegistryStore'
import { FEATURE_SUBROUTES } from '../product/routeRegistry'
import McpView from './McpView'
import MemoryPanel from './MemoryPanel'
import SkillPanel from './SkillPanel'

type WorkbenchTab = (typeof FEATURE_SUBROUTES)[number]['id']

function isWorkbenchTab(value: string | null): value is WorkbenchTab {
  return FEATURE_SUBROUTES.some((tab) => tab.id === value)
}

export default function FeaturesView() {
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const requestedTab = searchParams.get('tab')
  const activeTab: WorkbenchTab = isWorkbenchTab(requestedTab) ? requestedTab : 'mcp'
  const commands = useCommandRegistry((state) => state.commands)
  const [commandQuery, setCommandQuery] = useState('')

  const filteredCommands = useMemo(() => {
    const query = commandQuery.trim().toLowerCase()
    if (!query) return commands
    return commands.filter((command) => (
      `${command.command} ${command.label} ${command.description} ${command.category}`.toLowerCase().includes(query)
    ))
  }, [commandQuery, commands])

  const openCommandInChat = (command: string) => {
    navigate(`/?slash=${encodeURIComponent(command)}`)
  }

  return (
    <main className="feature-workbench">
      <section
        id="feature-workbench-panel"
        className="feature-workbench-body"
        aria-label={tr(FEATURE_SUBROUTES.find((tab) => tab.id === activeTab)?.labelKey ?? 'Tools and knowledge')}
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
