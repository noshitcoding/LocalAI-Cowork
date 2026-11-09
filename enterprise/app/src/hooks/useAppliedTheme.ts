import { useEffect } from 'react'

import { useConfigStore } from '../stores/configStore'
import { useUiStore } from '../stores/uiStore'
import { writeAuditEvent } from '../utils/audit'

type AppliedThemeOptions = {
  audit?: boolean
}

export function useAppliedTheme({ audit = false }: AppliedThemeOptions = {}) {
  const theme = useUiStore((state) => state.theme)
  const setTheme = useUiStore((state) => state.setTheme)
  const preferences = useConfigStore((state) => state.preferences)

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    document.body.dataset.theme = theme
    document.documentElement.style.fontSize = `${Math.max(85, Math.min(120, preferences.fontScale))}%`
    document.body.classList.toggle('compact-mode', preferences.compactMode)
  }, [preferences.compactMode, preferences.fontScale, theme])

  useEffect(() => {
    if (!preferences.syncThemeWithSystem) return
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const applyTheme = (matchesDark: boolean) => setTheme(matchesDark ? 'dark' : 'light')
    applyTheme(media.matches)

    const onChange = (event: MediaQueryListEvent) => applyTheme(event.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [preferences.syncThemeWithSystem, setTheme])

  useEffect(() => {
    if (!audit) return
    void writeAuditEvent('ui', 'theme_applied', { theme })
  }, [audit, theme])

  return theme
}
