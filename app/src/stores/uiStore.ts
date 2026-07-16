import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type AppMode = 'work' | 'settings' | 'crew'
export type WorkingPathKind = 'file' | 'folder'
export type ThemeMode = 'light' | 'dark'

export const LEFT_SIDEBAR_DEFAULT_WIDTH = 320
export const LEFT_SIDEBAR_MIN_WIDTH = 240
export const LEFT_SIDEBAR_MAX_WIDTH = 520

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
  commandPaletteOpen: boolean
  shortcutsOverlayOpen: boolean
  setActiveMode: (mode: AppMode) => void
  setWorkingPath: (path: string | null, kind?: WorkingPathKind | null) => void
  setWorkingFolder: (folder: string | null) => void
  toggleLeftSidebar: () => void
  setLeftSidebarWidth: (width: number) => void
  setTheme: (theme: ThemeMode) => void
  toggleTheme: () => void
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
      setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
      setShortcutsOverlayOpen: (open) => set({ shortcutsOverlayOpen: open }),
    }),
    {
      name: 'open-cowork-ui',
      partialize: (state) => ({
        activeMode: state.activeMode,
        leftSidebarOpen: state.leftSidebarOpen,
        leftSidebarWidth: state.leftSidebarWidth,
        theme: state.theme,
      }),
    }
  )
)
