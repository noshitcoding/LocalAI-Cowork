import {describe, expect, it} from "vitest";
import {appearanceScope, appearanceStorageKey, defaultAppearancePreferences, normalizeAppearancePreferences, readAppearancePreferences, saveAppearancePreferences} from "./appearancePreferences";

function memoryStorage() {const values = new Map<string, string>(); return {getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => {values.set(key, value);}};}
describe("device appearance preferences", () => {
  it("persists actual preferences independently for local and server endpoints", () => {
    const storage = memoryStorage(), local = appearanceScope({base: ""}), server = appearanceScope({base: "https://cowork.example"});
    saveAppearancePreferences(storage, local, {...defaultAppearancePreferences, fontSize: 18, compactMode: true, showTimestamps: false, sendShortcut: "ctrl_enter"});
    expect(readAppearancePreferences(storage, local)).toMatchObject({fontSize: 18, compactMode: true, showTimestamps: false, sendShortcut: "ctrl_enter"});
    expect(readAppearancePreferences(storage, server)).toEqual(defaultAppearancePreferences);
    expect(appearanceStorageKey(server)).not.toContain("https://");
  });
  it("recovers corrupted storage and ignores invalid or unknown values", () => {
    const storage = memoryStorage(); storage.setItem(appearanceStorageKey("broken"), "{broken");
    expect(readAppearancePreferences(storage, "broken")).toEqual(defaultAppearancePreferences);
    expect(normalizeAppearancePreferences({fontSize: 999, compactMode: "true", showTimestamps: false, focusMode: true, sendShortcut: "other", secret: "ignored"})).toEqual({...defaultAppearancePreferences, showTimestamps: false, focusMode: true});
    expect(normalizeAppearancePreferences({fontSize: 15.5}).fontSize).toBe(14);
  });
  it("surfaces a failed persistence operation instead of claiming a saved preference", () => {
    expect(() => saveAppearancePreferences({getItem: () => null, setItem: () => {throw new Error("quota");}}, "local", {...defaultAppearancePreferences})).toThrow("quota");
  });
});
