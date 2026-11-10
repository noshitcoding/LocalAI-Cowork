import {useCallback, useSyncExternalStore} from "react";
import type {Client} from "@cowork/client";

export type AppearancePreferences = {
  fontSize: number;
  compactMode: boolean;
  focusMode: boolean;
  showTimestamps: boolean;
  sendShortcut: "enter" | "ctrl_enter";
  reduceMotion: boolean;
  followSystemTheme:boolean;
  showThinking:boolean;
  confirmOnClose:boolean;
  notificationsEnabled:boolean;
};
export const defaultAppearancePreferences: Readonly<AppearancePreferences> = Object.freeze({
  fontSize: 14, compactMode: false, focusMode: false, showTimestamps: true,
  sendShortcut: "enter", reduceMotion: false,followSystemTheme:false,showThinking:true,confirmOnClose:true,notificationsEnabled:false,
});
type StorageLike = Pick<Storage, "getItem" | "setItem">;
const prefix = "cowork-v2:appearance:v1:";
export const appearanceScope = (client: Pick<Client, "base">) => client.base || "local";
export const appearanceStorageKey = (scope: string) => prefix + encodeURIComponent(scope);

export function normalizeAppearancePreferences(value: unknown): AppearancePreferences {
  const source = value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
  const bool = (key: keyof AppearancePreferences) => typeof source[key] === "boolean" ? source[key] as boolean : defaultAppearancePreferences[key] as boolean;
  return {
    fontSize: typeof source.fontSize === "number" && Number.isInteger(source.fontSize) && source.fontSize >= 12 && source.fontSize <= 22 ? source.fontSize : 14,
    compactMode: bool("compactMode"), focusMode: bool("focusMode"), showTimestamps: bool("showTimestamps"),
    sendShortcut: source.sendShortcut === "ctrl_enter" ? "ctrl_enter" : "enter", reduceMotion: bool("reduceMotion"),followSystemTheme:bool("followSystemTheme"),showThinking:bool("showThinking"),confirmOnClose:bool("confirmOnClose"),notificationsEnabled:bool("notificationsEnabled"),
  };
}
export function readAppearancePreferences(storage: Pick<StorageLike, "getItem">, scope: string): AppearancePreferences {
  try { return normalizeAppearancePreferences(JSON.parse(storage.getItem(appearanceStorageKey(scope)) ?? "null")); }
  catch { return {...defaultAppearancePreferences}; }
}
export function saveAppearancePreferences(storage: StorageLike, scope: string, value: AppearancePreferences): AppearancePreferences {
  const next = normalizeAppearancePreferences(value);
  storage.setItem(appearanceStorageKey(scope), JSON.stringify(next));
  return next;
}

const snapshots = new Map<string, AppearancePreferences>();
const listeners = new Set<() => void>();
function getSnapshot(scope: string) {
  if (!snapshots.has(scope)) snapshots.set(scope, typeof localStorage === "undefined" ? {...defaultAppearancePreferences} : readAppearancePreferences(localStorage, scope));
  return snapshots.get(scope)!;
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  const changed = (event: StorageEvent) => {
    if (event.key === null || event.key.startsWith(prefix)) {
      snapshots.clear();
      for (const notify of listeners) notify();
    }
  };
  if (typeof window !== "undefined") window.addEventListener("storage", changed);
  return () => { listeners.delete(listener); if (typeof window !== "undefined") window.removeEventListener("storage", changed); };
}
export function useAppearancePreferences(scope: string) {
  const preferences = useSyncExternalStore(subscribe, useCallback(() => getSnapshot(scope), [scope]), () => defaultAppearancePreferences as AppearancePreferences);
  const setPreferences = useCallback((patch: Partial<AppearancePreferences>) => {
    const next = normalizeAppearancePreferences({...getSnapshot(scope), ...patch});
    // Storage errors are surfaced to the settings form; a failed save is not reported as applied.
    if (typeof localStorage !== "undefined") saveAppearancePreferences(localStorage, scope, next);
    snapshots.set(scope, next);
    for (const notify of listeners) notify();
  }, [scope]);
  return {preferences, setPreferences};
}

export function appearanceAttributes(preferences: AppearancePreferences) {
  return {"data-cowork-compact": String(preferences.compactMode), "data-cowork-focus": String(preferences.focusMode),
    "data-cowork-timestamps": String(preferences.showTimestamps), "data-cowork-motion": preferences.reduceMotion ? "reduced" : "system", "data-cowork-thinking":String(preferences.showThinking)};
}
export function applyAppearancePreferences(document: Document, preferences: AppearancePreferences) {
  const root = document.documentElement;
  root.style.setProperty("--cowork-chat-font-size", `${preferences.fontSize}px`);
  for (const [key, value] of Object.entries(appearanceAttributes(preferences))) root.setAttribute(key, value);
}
