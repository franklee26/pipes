import { describe, expect, it } from "vitest";
import { makeLog } from "../test/fixtures";
import { RunOutputBuffer } from "./runOutput";

describe("bounded terminal text processing", () => {
  it("renders partial output immediately and joins fragments without inventing newlines", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("hel")]);
    expect(buffer.snapshot().lines).toMatchObject([{ text: "hel", complete: false }]);
    buffer.append([makeLog("lo\nnext", 2)]);
    expect(buffer.snapshot().lines.map((line) => [line.text, line.complete])).toEqual([["hello", true], ["next", false]]);
  });

  it("strips fragmented ANSI, OSC hyperlinks, terminal titles and control bytes", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("\u001b[3")]);
    buffer.append([makeLog("1mred\u001b[0m \u001b]8;;https://invalid.test", 2)]);
    buffer.append([makeLog("\u001b\\link\u001b]8;;\u0007 \u001b]0;hidden title", 3)]);
    buffer.append([makeLog("\u0007safe\u0000\u0007\u007f\u001b(B\n\u009b32mgreen\u009b0m", 4)]);
    expect(buffer.snapshot().lines.map((line) => line.text)).toEqual(["red link safe", "green"]);
    expect(JSON.stringify(buffer.snapshot())).not.toContain("\\u001b");
  });

  it("replaces CR progress updates and preserves CRLF across page boundaries", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("Progress 100%\r")]);
    expect(buffer.snapshot().lines[0]?.text).toBe("Progress 100%");
    buffer.append([makeLog("Done\r", 2)]);
    expect(buffer.snapshot().lines[0]?.text).toBe("Done");
    buffer.append([makeLog("\nsecond\r\u001b[2Kcomplete\n", 3)]);
    expect(buffer.snapshot().lines.map((line) => line.text)).toEqual(["Done", "complete"]);
  });

  it("retains Unicode and split surrogate pairs without losing interleaved attribution", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("cafe\u0301 \ud83c")]);
    buffer.append([makeLog("warning", 2, { stream: "stderr", stageId: "beta", commandId: "unit" })]);
    buffer.append([makeLog("\udf0d\n", 3)]);
    expect(buffer.snapshot().lines.map((line) => [line.stageId, line.stream, line.text])).toEqual([
      ["test", "stdout", "cafe\u0301 🌍"], ["beta", "stderr", "warning"],
    ]);
  });

  it("handles backspace and incomplete trailing Unicode safely", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("ab🌍\bX\ud83c")]);
    expect(buffer.snapshot().lines[0]?.text).toBe("abX");
    buffer.finish();
    expect(buffer.snapshot().lines[0]?.text).toBe("abX\ufffd");
  });

  it("keeps arbitrary markup as text rather than treating output as a document", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("<script>alert('x')</script><img src=x onerror=alert(1)>")]);
    expect(buffer.snapshot().lines[0]?.text).toBe("<script>alert('x')</script><img src=x onerror=alert(1)>");
  });

  it("bounds both line count and long unterminated output, without cutting a surrogate pair", () => {
    const buffer = new RunOutputBuffer(3, 12);
    buffer.append([makeLog("one\ntwo\nthree\nfour\n")]);
    expect(buffer.snapshot().lines).toHaveLength(3);
    expect(buffer.snapshot().discardedLines).toBe(1);
    buffer.append([makeLog("prefix".repeat(10) + "🌍🌍🌍🌍🌍🌍", 2)]);
    const snapshot = buffer.snapshot();
    expect(snapshot.lines.reduce((count, line) => count + line.text.length, 0)).toBeLessThanOrEqual(12);
    expect(snapshot.lines[0]?.text).toBe("🌍🌍🌍🌍🌍🌍");
    expect(snapshot.discardedCharacters).toBeGreaterThan(0);
  });

  it("retains parser state when an earlier partial line leaves the in-memory tail", () => {
    const buffer = new RunOutputBuffer(1, 100);
    buffer.append([makeLog("earlier\u001b[")]);
    buffer.append([makeLog("other\n", 2, { stream: "stderr" })]);
    buffer.append([makeLog("32mcontinued\u001b[0m", 3)]);
    expect(buffer.snapshot().lines.map((line) => line.text)).toEqual(["continued"]);
    expect(buffer.snapshot().discardedLines).toBe(2);
  });
});
