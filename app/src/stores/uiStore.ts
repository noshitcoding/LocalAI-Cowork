import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type AppMode = 'work' | 'settings' | 'crew'
export type WorkingPathKind = 'file' | 'folder'
export type ThemeMode = 'light' | 'dark'

export const LEFT_SIDEBAR_DEFAULT_WIDTH = 260
export const LEFT_SIDEBAR_MIN_WIDTH = 220
export const LEFT_SIDEBAR_MAX_WIDTH = 360

export function clampLeftSidebarWidth(width: number): number {
  if (!Number.isFinite(width)) return LEFT_SIDEBAR_DEFAULT_WIDTH
  return Math.min(LEFT_SIDEBAR_MAX_WIDTH, Math.max(LEFT_SIDEBAR_MIN_WIDTH, Math.round(width)))
}

type UiState = {
  activeMode: AppMode
  workingFolder: string | null
  workingPathKind: WorkingPathKind | null
  leftSidebarOpen: boolean
  leftSidebarWidth: number
  theme: ThemeMode
  appMenuOpen: boolean
  appMenuSearchFocused: boolean
  contextDrawerOpen: boolean
  commandPaletteOpen: boolean
  shortcutsOverlayOpen: boolean
  setActiveMode: (mode: AppMode) => void
  setWorkingPath: (path: string | null, kind?: WorkingPathKind | null) => void
  setWorkingFolder: (folder: string | null) => void
  toggleLeftSidebar: () => void
  setLeftSidebarWidth: (width: number) => void
  setTheme: (theme: ThemeMode) => void
  toggleTheme: () => void
  setAppMenuOpen: (open: boolean, focusSearch?: boolean) => void
  setAppMenuSearchFocused: (focused: boolean) => void
  setContextDrawerOpen: (open: boolean) => void
  closeShellOverlays: () => void
  setCommandPaletteOpen: (open: boolean) => void
  setShortcutsOverlayOpen: (open: boolean) => void
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      activeMode: 'work',
      workingFolder: null,
      workingPathKind: null,
      leftSidebarOpen: true,
      leftSidebarWidth: LEFT_SIDEBAR_DEFAULT_WIDTH,
      theme: 'light',
      appMenuOpen: false,
      appMenuSearchFocused: false,
      contextDrawerOpen: false,
      commandPaletteOpen: false,
      shortcutsOverlayOpen: false,
      setActiveMode: (mode) => set({ activeMode: mode }),
      setWorkingPath: (path, kind = null) =>
        set({ workingFolder: path, workingPathKind: path ? kind : null }),
      setWorkingFolder: (folder) =>
        set({ workingFolder: folder, workingPathKind: folder ? 'folder' : null }),
      toggleLeftSidebar: () =>
        set((state) => ({ leftSidebarOpen: !state.leftSidebarOpen })),
      setLeftSidebarWidth: (width) =>
        set({ leftSidebarWidth: clampLeftSidebarWidth(width) }),
      setTheme: (theme) => set({ theme }),
      toggleTheme: () =>
        set((state) => ({ theme: state.theme === 'light' ? 'dark' : 'light' })),
      setAppMenuOpen: (open, focusSearch = false) =>
        set((state) => ({
          appMenuOpen: open,
          appMenuSearchFocused: open && focusSearch,
          contextDrawerOpen: open ? false : state.contextDrawerOpen,
          commandPaletteOpen: open && focusSearch,
        })),
      setAppMenuSearchFocused: (focused) => set({ appMenuSearchFocused: focused }),
      setContextDrawerOpen: (open) =>
        set((state) => ({
          contextDrawerOpen: open,
          appMenuOpen: open ? false : state.appMenuOpen,
          appMenuSearchFocused: false,
          commandPaletteOpen: false,
          shortcutsOverlayOpen: false,
        })),
      closeShellOverlays: () => set({
        appMenuOpen: false,
        appMenuSearchFocused: false,
        contextDrawerOpen: false,
        commandPaletteOpen: false,
        shortcutsOverlayOpen: false,
      }),
      setCommandPaletteOpen: (open) =>
        set((state) => ({
          commandPaletteOpen: open,
          appMenuOpen: open,
          appMenuSearchFocused: open,
          contextDrawerOpen: open ? false : state.contextDrawerOpen,
        })),
      setShortcutsOverlayOpen: (open) =>
        set((state) => ({
          shortcutsOverlayOpen: open,
          appMenuOpen: open ? true : state.appMenuOpen,
          appMenuSearchFocused: false,
          contextDrawerOpen: open ? false : state.contextDrawerOpen,
        })),
    }),
    {
      name: 'open-cowork-ui',
      partialize: (state) => ({
        activeMode: state.activeMode,
        leftSidebarOpen: state.leftSidebarOpen,
        leftSidebarWidth: state.leftSidebarWidth,
        theme: state.theme,
      }),
      merge: (persistedState, currentState) => {
        const persisted = persistedState as Partial<UiState>
        return {
          ...currentState,
          ...persisted,
          leftSidebarWidth: clampLeftSidebarWidth(
            persisted.leftSidebarWidth ?? currentState.leftSidebarWidth,
          ),
        }
      },
    }
  )
)
