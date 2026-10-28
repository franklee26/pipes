import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { RunOutputBuffer } from "../lib/runOutput";
import { makeLog, makeRun } from "../test/fixtures";
import { RunTerminal } from "./RunTerminal";

describe("safe terminal output", () => {
  it("renders output as text, strips escapes, replaces progress and displays partial lines", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("\u001b[31mProgress 100%\rDone\u001b[0m\n<img src=x onerror=alert(1)>")]);
    const { container } = render(<RunTerminal run={makeRun()} output={buffer.snapshot()} cursor={1} stageFilter="" onStageFilter={vi.fn()} />);
    const terminal = screen.getByRole("region", { name: "Command output text" });
    expect(terminal).toHaveTextContent("Done");
    expect(terminal).not.toHaveTextContent("Progress");
    expect(terminal).toHaveTextContent("<img src=x onerror=alert(1)>");
    expect(container.querySelector("img")).toBeNull();
    expect(terminal.textContent).not.toContain("\u001b");
    expect(container.querySelector('[data-complete="false"]')).toHaveTextContent("<img");
    expect(terminal).toHaveAttribute("aria-live", "off");
    expect(terminal).toHaveTextContent("[test/unit] stdout");
  });

  it("allows auto-follow to be paused and filters stdout, stderr and commands", async () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("out\n"), makeLog("err\n", 2, { stream: "stderr" })]);
    const user = userEvent.setup();
    render(<RunTerminal run={makeRun()} output={buffer.snapshot()} cursor={2} stageFilter="" onStageFilter={vi.fn()} />);
    await user.click(screen.getByRole("checkbox", { name: "auto-follow on" }));
    expect(screen.getByRole("checkbox", { name: "auto-follow off" })).not.toBeChecked();
    await user.selectOptions(screen.getByRole("combobox", { name: "Stream" }), "stderr");
    const terminal = screen.getByRole("region", { name: "Command output text" });
    expect(terminal).toHaveTextContent("err");
    expect(within(terminal).queryByText("out")).not.toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: "Command" }), JSON.stringify(["test", "unit"]));
    expect(terminal).toHaveTextContent("[test/unit] stderr");
  });

  it("clearly indicates tail truncation while keeping filters and saved logs inspectable", () => {
    const buffer = new RunOutputBuffer(1, 100);
    buffer.append([makeLog("first\nlast\n")]);
    render(<RunTerminal run={makeRun({ status: "passed" })} output={buffer.snapshot()} cursor={1} stageFilter="" onStageFilter={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent("bounded tail of the saved log");
    expect(screen.getByRole("status")).toHaveTextContent("1 lines");
    expect(screen.getByRole("region", { name: "Command output text" })).toHaveTextContent("last");
    expect(screen.getByRole("heading", { name: "output / saved" })).toBeVisible();
    expect(screen.getByText(/Persisted output is capped at 64 MiB/)).toBeVisible();
  });

  it("renders backend retention markers as attributed system output", () => {
    const buffer = new RunOutputBuffer();
    buffer.append([makeLog("Retained output limit reached; further command output is truncated.\n", 1, { stageId: null, commandId: null, stream: "system" })]);
    render(<RunTerminal run={makeRun()} output={buffer.snapshot()} cursor={1} stageFilter="" onStageFilter={vi.fn()} />);
    const terminal = screen.getByRole("region", { name: "Command output text" });
    expect(terminal).toHaveTextContent("[runner] system");
    expect(terminal).toHaveTextContent("Retained output limit reached; further command output is truncated.");
  });
});
