import {useEffect, useRef, useState} from "react";
import {Mic, MicOff} from "lucide-react";

type RecognitionResult = {isFinal: boolean; 0: {transcript: string}};
type RecognitionEvent = {resultIndex: number; results: ArrayLike<RecognitionResult>};
type Recognition = {
  lang: string; continuous: boolean; interimResults: boolean;
  start(): void; stop(): void; abort(): void;
  onresult: ((event: RecognitionEvent) => void) | null;
  onerror: ((event: {error: string}) => void) | null;
  onend: (() => void) | null;
};
type RecognitionConstructor = new () => Recognition;

function recognitionConstructor(): RecognitionConstructor | undefined {
  if (typeof window === "undefined") return undefined;
  const scope = window as unknown as {SpeechRecognition?: RecognitionConstructor; webkitSpeechRecognition?: RecognitionConstructor};
  return scope.SpeechRecognition ?? scope.webkitSpeechRecognition;
}

/** Appends dictated text to a draft without gluing words together. */
export function appendTranscript(draft: string, text: string): string {
  const spoken = text.trim();
  if (!spoken) return draft;
  return draft && !/\s$/.test(draft) ? draft + " " + spoken : draft + spoken;
}

const windowsHost = () => typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

/**
 * Dictation through the platform speech recognizer. Where the window offers none
 * (for example the Windows desktop shell), it points to Windows voice typing instead.
 */
export function SpeechInputButton({language, disabled, onText, onHint, focusTarget}: {
  language: "de" | "en"; disabled?: boolean; onText: (text: string) => void; onHint: (hint: string) => void; focusTarget?: () => void;
}) {
  const de = language === "de";
  const [listening, setListening] = useState(false);
  const recognition = useRef<Recognition | null>(null);
  useEffect(() => () => recognition.current?.abort(), []);
  function unsupported() {
    focusTarget?.();
    onHint(windowsHost()
      ? (de ? "Diese Oberfläche hat keine eingebaute Spracherkennung. Drücke Win + H für die Windows-Spracheingabe – der Text landet direkt im Eingabefeld." : "This window has no built-in speech recognition. Press Win + H for Windows voice typing; the text goes straight into the input field.")
      : (de ? "Dieser Browser unterstützt keine Spracheingabe." : "This browser does not support voice input."));
  }
  function toggle() {
    if (listening) {recognition.current?.stop(); return;}
    const Constructor = recognitionConstructor();
    if (!Constructor) {unsupported(); return;}
    let session: Recognition, failed = false;
    try {session = new Constructor();} catch {unsupported(); return;}
    session.lang = de ? "de-DE" : "en-US";
    session.continuous = true;
    session.interimResults = false;
    session.onresult = event => {
      for (let index = event.resultIndex; index < event.results.length; index++) {
        const result = event.results[index];
        if (result.isFinal) onText(result[0].transcript);
      }
    };
    session.onerror = event => {
      if (event.error === "no-speech" || event.error === "aborted") return;
      failed = true;
      if (event.error === "not-allowed" || event.error === "service-not-allowed") onHint(de ? "Der Mikrofonzugriff wurde verweigert." : "Microphone access was denied.");
      else if (event.error === "network" || event.error === "language-not-supported") unsupported();
      else onHint(de ? "Die Spracheingabe wurde unterbrochen." : "Voice input was interrupted.");
    };
    session.onend = () => {setListening(false); if (recognition.current === session) recognition.current = null; if (!failed) onHint("");};
    recognition.current = session;
    try {session.start(); setListening(true); onHint(de ? "Spracheingabe aktiv – sprich jetzt. Erneut klicken zum Beenden." : "Listening – speak now. Click again to stop.");}
    catch {recognition.current = null; unsupported();}
  }
  const label = listening ? (de ? "Spracheingabe beenden" : "Stop voice input") : (de ? "Spracheingabe" : "Voice input");
  return <button type="button" className={"icon composer-mic " + (listening ? "listening" : "")} aria-label={label} title={label} aria-pressed={listening} disabled={disabled && !listening} onClick={toggle}>
    {listening ? <MicOff size={18}/> : <Mic size={18}/>}
  </button>;
}
