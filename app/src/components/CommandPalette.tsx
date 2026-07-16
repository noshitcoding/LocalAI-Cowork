import { useCallback, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { tr } from '../i18n'
import { useCommandRegistry, type SlashCommandCategory } from '../stores/commandRegistryStore'
import { useUiStore } from '../stores/uiStore'
import { PRODUCT_ROUTES } from '../product/routeRegistry'

const CATEGORY_LABELS: Record<SlashCommandCategory, string> = {
  navigation: 'Navigation',
  workspace: 'Workspace',
  agent: 'Agent',
  model: 'Model',
  memory: 'Memory',
  tools: 'Tools',
  session: 'Session',
  config: 'Configuration',
  security: 'Security',
  display: 'Display',
  plugins: 'Plugins',
  crew: 'Crew AI',
  debug: 'Debug',
  export: 'Export',
}

const CATEGORY_ORDER: SlashCommandCategory[] = [
  'navigation',
  'workspace',
  'agent',
  'crew',
  'model',
  'memory',
  'session',
  'tools',
  'plugins',
  'config',
  'security',
  'display',
  'export',
  'debug',
]

export default function CommandPalette() {
  const navigate = useNavigate()
  const {
    commandPaletteOpen,
    setCommandPaletteOpen,
    toggleLeftSidebar,
    leftSidebarOpen,
    toggleTheme,
    theme,
    setShortcutsOverlayOpen,
    setActiveMode,
  } = useUiStore()
  const { commands: registeredCommands, executeCommand } = useCommandRegistry()
  const [query, setQuery] = useState('')
  const [selectedCategory, setSelectedCategory] = useState<SlashCommandCategory | null>(null)

  const legacyCommands = useMemo(
    () => [
      ...PRODUCT_ROUTES.map((route) => ({
        id: route.commandId,
        label: route.commandLabel,
        hint: route.shortcut,
        action: () => {
          if (route.activeMode) {
            setActiveMode(route.activeMode)
          }
          navigate(route.path)
        },
      })),
      { id: 'toggle-left-sidebar', label: leftSidebarOpen ? 'Hide sidebar' : 'Show sidebar', hint: 'Ctrl+Shift+B', action: () => toggleLeftSidebar() },
      { id: 'show-shortcuts', label: 'Show shortcut overview', hint: 'Ctrl+Shift+?', action: () => setShortcutsOverlayOpen(true) },
      { id: 'toggle-theme', label: theme === 'light' ? 'Enable dark theme' : 'Enable light theme', hint: 'Ctrl+Shift+L', action: () => toggleTheme() },
    ],
    [leftSidebarOpen, navigate, setActiveMode, setShortcutsOverlayOpen, theme, toggleLeftSidebar, toggleTheme],
  )

  const isSlashQuery = query.startsWith('/')

  const filteredCommands = useMemo(() => {
    if (isSlashQuery) {
      const normalizedQuery = query.toLowerCase()
      let filtered = registeredCommands.filter(cmd =>
        cmd.command.toLowerCase().includes(normalizedQuery)
        || tr(cmd.label).toLowerCase().includes(normalizedQuery.slice(1))
        || tr(cmd.description).toLowerCase().includes(normalizedQuery.slice(1)),
      )
      if (selectedCategory) {
        filtered = filtered.filter(cmd => cmd.category === selectedCategory)
      }
      return filtered
    }

    const normalizedQuery = query.trim().toLowerCase()
    if (!normalizedQuery) {
      if (selectedCategory) {
        return registeredCommands.filter(cmd => cmd.category === selectedCategory)
      }
      return [
        ...legacyCommands.map(c => ({
          ...c,
          command: '',
          description: c.label,
          category: 'navigation' as SlashCommandCategory,
          execute: c.action,
        })),
        ...registeredCommands,
      ]
    }

    const matchingRegistry = registeredCommands.filter(cmd =>
      tr(cmd.label).toLowerCase().includes(normalizedQuery)
      || cmd.command.toLowerCase().includes(normalizedQuery)
      || tr(cmd.description).toLowerCase().includes(normalizedQuery),
    )
    const matchingLegacy = legacyCommands
      .filter(c => c.label.toLowerCase().includes(normalizedQuery) || c.hint.toLowerCase().includes(normalizedQuery))
      .map(c => ({
        ...c,
        command: '',
        description: c.label,
        category: 'navigation' as SlashCommandCategory,
        execute: c.action,
      }))

    return [...matchingLegacy, ...matchingRegistry]
  }, [query, isSlashQuery, selectedCategory, registeredCommands, legacyCommands])

  const groupedCommands = useMemo(() => {
    if (isSlashQuery || selectedCategory) return null
    const groups = new Map<SlashCommandCategory, typeof filteredCommands>()
    for (const cmd of filteredCommands) {
      const cat = cmd.category
      if (!groups.has(cat)) groups.set(cat, [])
      groups.get(cat)!.push(cmd)
    }
    return groups
  }, [filteredCommands, isSlashQuery, selectedCategory])

  const closePalette = useCallback(() => {
    setCommandPaletteOpen(false)
    setSelectedCategory(null)
  }, [setCommandPaletteOpen])

  const handleExecute = useCallback(async (cmd: typeof filteredCommands[0]) => {
    const slashArgs = isSlashQuery ? query.replace(/^\/\S+\s*/, '').trim() : undefined
    if ('action' in cmd && typeof cmd.action === 'function') {
      (cmd as { action: () => void }).action()
    } else {
      await executeCommand(cmd.id, slashArgs || undefined)
    }
    setCommandPaletteOpen(false)
    setQuery('')
    setSelectedCategory(null)
  }, [isSlashQuery, query, executeCommand, setCommandPaletteOpen])

  if (!commandPaletteOpen) return null

  return (
    <div className="command-palette-overlay">
      <button
        type="button"
        className="command-palette-backdrop"
        aria-label={tr('Close command palette')}
        onClick={closePalette}
      />
      <div
        className="command-palette"
        data-doc-id="element:/app/command-palette"
        role="dialog"
        aria-modal="true"
        aria-label={tr('Command palette')}
      >
        <div className="command-palette-header">
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={isSlashQuery ? tr('Enter slash command...') : tr('Search command or / for commands...')}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && filteredCommands.length > 0) {
                handleExecute(filteredCommands[0])
              }
            }}
          />
          <button type="button" data-doc-id="button:/app/command-palette/close" onClick={closePalette}>{tr('Esc')}</button>
        </div>

        <div className="command-palette-categories" aria-label={tr("Command categories")}>
          <button
            type="button"
            className={`command-palette-category${selectedCategory === null ? ' active' : ''}`}
            data-doc-id="button:/app/command-palette/select-category"
            onClick={() => setSelectedCategory(null)}
          >
            {tr('All')}
          </button>
          {CATEGORY_ORDER.map(cat => (
            <button
              key={cat}
              type="button"
              className={`command-palette-category${selectedCategory === cat ? ' active' : ''}`}
              data-doc-id="button:/app/command-palette/select-category"
              onClick={() => setSelectedCategory(selectedCategory === cat ? null : cat)}
            >
              {tr(CATEGORY_LABELS[cat])}
            </button>
          ))}
        </div>

        <ul className="command-palette-list">
          {groupedCommands && !query.trim() ? (
            Array.from(groupedCommands.entries())
              .sort(([a], [b]) => CATEGORY_ORDER.indexOf(a) - CATEGORY_ORDER.indexOf(b))
              .map(([cat, cmds]) => (
                <li key={cat} className="command-palette-group">
                  <div className="command-palette-group-label">
                    {tr(CATEGORY_LABELS[cat])}
                  </div>
                  <ul className="command-palette-group-list">
                    {cmds.slice(0, 5).map(cmd => (
                      <li key={cmd.id}>
                        <button type="button" data-doc-id="button:/app/command-palette/execute-command" onClick={() => handleExecute(cmd)}>
                          <span>
                            {cmd.command && <span className="command-palette-command">{cmd.command}</span>}
                            {tr(cmd.label)}
                          </span>
                          <span className="command-palette-description compact">
                            {tr(cmd.description)}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </li>
              ))
          ) : (
            filteredCommands.map(cmd => (
              <li key={cmd.id}>
                <button type="button" data-doc-id="button:/app/command-palette/execute-command" onClick={() => handleExecute(cmd)}>
                  <span>
                    {cmd.command && <span className="command-palette-command">{cmd.command}</span>}
                    {tr(cmd.label)}
                  </span>
                  <span className="command-palette-description">
                    {tr(cmd.description)}
                  </span>
                </button>
              </li>
            ))
          )}
          {filteredCommands.length === 0 && (
            <li className="command-palette-empty">{tr('No results for')} "{query}"</li>
          )}
          <li className="command-palette-footer">
            {registeredCommands.length} {tr('Commands available - type / for slash commands')}
          </li>
        </ul>
      </div>
    </div>
  )
}
