import {describe, expect, it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {SpeechInputButton, appendTranscript} from "./SpeechInput";

describe("voice input", () => {
  it("appends dictated text with exactly one separating space", () => {
    expect(appendTranscript("", " Hallo Welt ")).toBe("Hallo Welt");
    expect(appendTranscript("Fasse zusammen", "die Datei")).toBe("Fasse zusammen die Datei");
    expect(appendTranscript("Zeile eins\n", "Zeile zwei")).toBe("Zeile eins\nZeile zwei");
    expect(appendTranscript("Unverändert", "   ")).toBe("Unverändert");
  });
  it("renders a labelled, idle microphone toggle in both languages", () => {
    for (const [language, label] of [["de", "Spracheingabe"], ["en", "Voice input"]] as const) {
      const html = renderToStaticMarkup(<SpeechInputButton language={language} onText={() => {}} onHint={() => {}}/>);
      expect(html).toContain(`aria-label="${label}"`);
      expect(html).toContain('aria-pressed="false"');
    }
  });
});
