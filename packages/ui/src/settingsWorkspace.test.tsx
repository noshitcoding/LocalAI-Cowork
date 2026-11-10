import {describe, expect, it, vi} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {SettingsNavigation, normalizeSettingsSection, settingsSections} from "./SettingsWorkspace";
import {productHubPanelKey} from "./ProductHub";
vi.mock("@novnc/novnc", () => ({default: class {}}));

describe("settings navigation and project context", () => {
  it("groups thirteen settings categories under four headings with one current page in both languages", () => {
    for (const language of ["de", "en"] as const) {
      const markup = renderToStaticMarkup(<SettingsNavigation section="mcp" language={language} onSectionChange={() => {}}/>);
      expect(settingsSections).toHaveLength(13);
      expect(new Set(settingsSections).size).toBe(13);
      expect(markup.match(/<button /g)).toHaveLength(13);
      expect(markup.match(/class="settings-navigation-group"/g)).toHaveLength(4);
      expect(markup.match(/aria-current="page"/g)).toHaveLength(1);
      expect(markup).toContain(language === "de" ? "KI &amp; Modelle" : "AI &amp; models");
      expect(markup).toContain(language === "de" ? "Teams &amp; Server" : "Teams &amp; server");
      expect(markup).toContain(language === "de" ? "MCP &amp; Connectoren" : "MCP &amp; connectors");
      expect(markup).not.toContain("LiteLLM");
    }
  });
  it("resolves unknown deep links to general settings", () => {
    expect(normalizeSettingsSection("mcp")).toBe("mcp");
    expect(normalizeSettingsSection("ai")).toBe("ai");
    expect(normalizeSettingsSection("sandbox")).toBe("sandbox");
    expect(normalizeSettingsSection("legacy-workspace")).toBe("general");
  });
  it("resets project-bound editors without remounting a signed-in team when the chat changes", () => {
    const first = {project: "project-a", order: "chat-a"}, moved = {project: "project-b", order: "chat-b"};
    for (const tab of ["knowledge", "skills", "schedules", "tools"] as const) expect(productHubPanelKey(tab, first)).not.toBe(productHubPanelKey(tab, moved));
    expect(productHubPanelKey("teams", first)).toBe(productHubPanelKey("teams", moved));
    expect(productHubPanelKey("tools", first)).not.toBe(productHubPanelKey("tools", {...first, order: "chat-b"}));
  });
});
