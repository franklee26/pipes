import type { RunLogEntry } from "./types";

export const OUTPUT_LINE_LIMIT = 2000;
export const OUTPUT_CHARACTER_LIMIT = 256_000;

export interface OutputLine {
  id: number;
  sequence: number;
  recordedAt: string;
  stageId: string | null;
  commandId: string | null;
  stream: RunLogEntry["stream"];
  text: string;
  complete: boolean;
}

export interface OutputSnapshot {
  lines: OutputLine[];
  discardedLines: number;
  discardedCharacters: number;
}

type Source = Omit<RunLogEntry, "text">;
interface StreamState {
  source: Source;
  mode: "text" | "escape" | "csi" | "string" | "string-escape" | "charset";
  parameters: string;
  highSurrogate: string;
  carriageReturn: boolean;
  line?: OutputLine;
}

function sourceKey(source: Source): string {
  return JSON.stringify([source.stageId, source.commandId, source.stream]);
}

/** A bounded, plain-text stream view, not a PTY or an ANSI/HTML renderer. */
export class RunOutputBuffer {
  private streams = new Map<string, StreamState>();
  private lines: OutputLine[] = [];
  private characters = 0;
  private nextId = 1;
  private discardedLines = 0;
  private discardedCharacters = 0;

  constructor(
    private readonly lineLimit = OUTPUT_LINE_LIMIT,
    private readonly characterLimit = OUTPUT_CHARACTER_LIMIT,
  ) {}

  append(entries: readonly RunLogEntry[]): void {
    for (const entry of entries) {
      const { text, ...source } = entry;
      const key = sourceKey(source);
      let state = this.streams.get(key);
      if (!state) {
        state = { source, mode: "text", parameters: "", highSurrogate: "", carriageReturn: false };
        this.streams.set(key, state);
      }
      state.source = source;
      const chunk = state.highSurrogate + text;
      state.highSurrogate = "";
      for (let index = 0; index < chunk.length; index += 1) {
        const unit = chunk.charCodeAt(index);
        if (unit >= 0xd800 && unit <= 0xdbff) {
          if (index === chunk.length - 1) {
            state.highSurrogate = chunk[index]!;
            break;
          }
          const next = chunk.charCodeAt(index + 1);
          if (next >= 0xdc00 && next <= 0xdfff) {
            this.consume(state, chunk.slice(index, index + 2));
            index += 1;
          } else this.consume(state, "\ufffd");
        } else {
          this.consume(state, unit >= 0xdc00 && unit <= 0xdfff ? "\ufffd" : chunk[index]!);
        }
      }
      this.trim();
    }
  }

  finish(): void {
    for (const state of this.streams.values()) {
      if (state.highSurrogate) this.consume(state, "\ufffd");
      state.highSurrogate = "";
    }
    this.trim();
  }

  snapshot(): OutputSnapshot {
    return {
      lines: this.lines.map((line) => ({ ...line })),
      discardedLines: this.discardedLines,
      discardedCharacters: this.discardedCharacters,
    };
  }

  private lineFor(state: StreamState): OutputLine {
    if (!state.line) {
      state.line = { ...state.source, id: this.nextId++, text: "", complete: false };
      this.lines.push(state.line);
      if (this.lines.length > this.lineLimit) this.trim();
    }
    return state.line;
  }

  private clearLine(state: StreamState): void {
    if (!state.line) return;
    this.characters -= state.line.text.length;
    state.line.text = "";
  }

  private consume(state: StreamState, character: string): void {
    if (state.mode === "string" || state.mode === "string-escape") {
      if (character === "\u0007" || character === "\u009c" ||
        (state.mode === "string-escape" && character === "\\")) state.mode = "text";
      else state.mode = character === "\u001b" ? "string-escape" : "string";
      return;
    }
    if (character === "\u001b") {
      state.mode = "escape";
      return;
    }
    if (state.mode === "escape") {
      state.parameters = "";
      if (character === "[") state.mode = "csi";
      else if ("]P^_X".includes(character)) state.mode = "string";
      else if ("()*+-./%#".includes(character)) state.mode = "charset";
      else state.mode = "text";
      return;
    }
    if (state.mode === "charset") {
      state.mode = "text";
      return;
    }
    if (state.mode === "csi") {
      const code = character.charCodeAt(0);
      if (code >= 0x40 && code <= 0x7e) {
        if (character === "K" && (state.parameters === "2" || state.carriageReturn)) this.clearLine(state);
        if (character === "G" && (state.parameters === "" || state.parameters === "1")) state.carriageReturn = true;
        state.mode = "text";
        state.parameters = "";
      } else if (state.parameters.length < 32) state.parameters += character;
      return;
    }
    if (character === "\u009b") {
      state.mode = "csi";
      state.parameters = "";
      return;
    }
    if ("\u0090\u0098\u009d\u009e\u009f".includes(character)) {
      state.mode = "string";
      return;
    }
    if (character === "\r") {
      state.carriageReturn = true;
      return;
    }
    if (character === "\n") {
      this.lineFor(state).complete = true;
      state.line = undefined;
      state.carriageReturn = false;
      return;
    }
    if (character === "\b") {
      if (state.line?.text) {
        const text = state.line.text;
        const last = text.charCodeAt(text.length - 1);
        const count = last >= 0xdc00 && last <= 0xdfff ? 2 : 1;
        state.line.text = text.slice(0, -count);
        this.characters -= count;
      }
      return;
    }
    const code = character.charCodeAt(0);
    if ((code < 0x20 && character !== "\t") || (code >= 0x7f && code <= 0x9f)) return;
    // CRLF keeps the completed line; a bare CR replaces a progress line on its next text.
    if (state.carriageReturn) {
      this.clearLine(state);
      state.carriageReturn = false;
    }
    this.lineFor(state).text += character;
    this.characters += character.length;
    if (this.characters > this.characterLimit + 4096) this.trim();
  }

  private trim(): void {
    while (this.lines.length > this.lineLimit || (this.characters > this.characterLimit && this.lines.length > 1)) {
      const removed = this.lines.shift()!;
      this.characters -= removed.text.length;
      this.discardedLines += 1;
      this.discardedCharacters += removed.text.length;
      const state = this.streams.get(sourceKey(removed));
      if (state?.line === removed) state.line = undefined;
    }
    const first = this.lines[0];
    if (first && this.characters > this.characterLimit) {
      let count = this.characters - this.characterLimit;
      const boundary = first.text.charCodeAt(count);
      if (boundary >= 0xdc00 && boundary <= 0xdfff) count += 1;
      first.text = first.text.slice(count);
      this.characters -= count;
      this.discardedCharacters += count;
    }
  }
}
