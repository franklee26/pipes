import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { makeRun, registeredFixture, unitCommandFixture } from "../test/fixtures";
import type { PipelineRun } from "../lib/types";
import { StartRunDialog } from "./StartRunDialog";

function setup(confirm = vi.fn(async () => makeRun())) {
  const callbacks = { onConfirm: confirm, onStarted: vi.fn(), onClose: vi.fn(), onInvalidated: vi.fn() };
  const rendered = render(<StartRunDialog review={registeredFixture} {...callbacks} />);
  return { ...rendered, ...callbacks, user: userEvent.setup(), dialog: screen.getByRole("dialog") };
}

describe("explicit execution review", () => {
  it("states explicitly when commands have no default timeout", () => {
    const review = structuredClone(registeredFixture);
    review.manifest.execution.timeout = null;
    render(<StartRunDialog review={review} onConfirm={vi.fn()} onStarted={vi.fn()} onClose={vi.fn()} onInvalidated={vi.fn()} />);
    expect(screen.getByRole("dialog")).toHaveTextContent("default command timeoutnone / until exit or cancellation");
  });

  it("does not present rounded large numeric statuses as exact review values", () => {
    const command = {
      ...unitCommandFixture,
      result: { type: "jsonFile" as const, path: "result.json", pointer: "/status", success: 9_007_199_254_740_992, failure: false },
    };
    const review = structuredClone(registeredFixture);
    review.manifest.stages[0]!.checks = [command];
    review.manifest.stages[0]!.checkPlan = [{ kind: "command", command }];
    render(<StartRunDialog review={review} onConfirm={vi.fn()} onStarted={vi.fn()} onClose={vi.fn()} onInvalidated={vi.fn()} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("large integer / review original YAML");
    expect(dialog).toHaveTextContent("exceeds the browser's exact integer range");
    expect(dialog).not.toHaveTextContent("9007199254740992");
  });

  it("requires a reference and affirmative approval and sends the frozen hash and revision", async () => {
    const { user, dialog, onConfirm, onStarted } = setup();
    const submit = within(dialog).getByRole("button", { name: "approve & start run" });
    expect(onConfirm).not.toHaveBeenCalled();
    expect(submit).toBeDisabled();
    expect(dialog).toHaveTextContent(registeredFixture.manifest.sourcePath);
    expect(dialog).toHaveTextContent("source.snapshot: working-tree");
    expect(dialog).toHaveTextContent("Clean worktrees are required by default");
    expect(dialog).toHaveTextContent("review it, and re-register the manifest");
    expect(dialog).toHaveTextContent("This dialog never changes source policy");
    expect(dialog).toHaveTextContent("Keychain secret references are not supported");
    expect(dialog).toHaveTextContent("Only one active run may use the same source directory within this registry");
    expect(dialog).toHaveTextContent("All check leaves are required");
    expect(dialog).toHaveTextContent("Prepare commands run sequentially and must all pass before any stage begins");
    expect(dialog).toHaveTextContent("A failed branch does not cancel other independent branches");
    expect(dialog).toHaveTextContent("BEFORE");
    expect(dialog).toHaveTextContent("result: exit code / success: 0 (default)");
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    expect(submit).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(submit);
    expect(onConfirm).toHaveBeenCalledExactlyOnceWith({
      appId: "atlas", expectedManifestHash: registeredFixture.manifest.manifestHash,
      expectedUpdatedAt: registeredFixture.updatedAt, releaseRef: "v1.5.0", approved: true,
    });
    expect(onStarted).toHaveBeenCalledOnce();
  });

  it("requires fresh approval when the release reference changes and never runs on cancel", async () => {
    const { user, dialog, onConfirm, onClose } = setup();
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "candidate-a");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "-changed");
    expect(within(dialog).getByRole("checkbox")).not.toBeChecked();
    expect(within(dialog).getByRole("button", { name: "approve & start run" })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("invalidates stale review errors, including a same-hash registration revision change", async () => {
    const { user, dialog, onConfirm, onInvalidated } = setup(vi.fn().mockRejectedValue({ code: "registration_changed", message: "The source path moved. Review again." }));
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "approve & start run" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The source path moved");
    expect(within(dialog).getByRole("checkbox")).not.toBeChecked();
    expect(within(dialog).getByRole("checkbox")).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "approve & start run" })).toBeDisabled();
    expect(onInvalidated).toHaveBeenCalledOnce();
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("shows backend failures without claiming that no run was accepted", async () => {
    const { user, dialog } = setup(vi.fn().mockRejectedValue({ code: "ipc_error", message: "Connection lost." }));
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "approve & start run" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Connection lost");
    expect(dialog).toHaveTextContent("check run history before retrying");
    expect(within(dialog).getByRole("checkbox")).not.toBeChecked();
  });

  it("suppresses duplicate submissions and ignores completion after unmount", async () => {
    let resolve!: (run: PipelineRun) => void;
    const confirm = vi.fn(() => new Promise<PipelineRun>((done) => { resolve = done; }));
    const { user, dialog, unmount, onStarted } = setup(confirm);
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    await user.click(within(dialog).getByRole("checkbox"));
    const form = within(dialog).getByRole("textbox", { name: "Release reference" }).closest("form")!;
    fireEvent.submit(form);
    fireEvent.submit(form);
    expect(confirm).toHaveBeenCalledOnce();
    expect(within(dialog).getByRole("button", { name: "starting..." })).toBeDisabled();
    unmount();
    await act(async () => resolve(makeRun()));
    expect(onStarted).not.toHaveBeenCalled();
  });

  it("shows typed JSON result contracts and per-command working directories", () => {
    const command = { ...unitCommandFixture, cwd: "checks", result: { type: "jsonFile" as const, path: "result.json", pointer: "/status", success: true, failure: false } };
    const review = structuredClone(registeredFixture);
    review.manifest.stages[0]!.checks = [command];
    review.manifest.stages[0]!.checkPlan = [{ kind: "command", command }];
    render(<StartRunDialog review={review} onConfirm={vi.fn()} onStarted={vi.fn()} onClose={vi.fn()} onInvalidated={vi.fn()} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("cwd: checks / source: /projects/atlas");
    expect(dialog).toHaveTextContent('result: JSON file result.json / pointer "/status"');
    expect(dialog).toHaveTextContent("success: true / failure: false (exact typed values)");
    expect(dialog).toHaveTextContent("no test counts are inferred");
    expect(dialog).toHaveTextContent("absolute paths are preserved");
  });
});
