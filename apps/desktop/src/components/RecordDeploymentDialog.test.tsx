import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { reviewConsumerDeployment } from "../lib/consumerDeployments";
import type { ConsumerDeployment, RecordConsumerDeployment } from "../lib/types";
import { consumerDeploymentFixture, consumerFixture, consumerRegisteredFixture } from "../test/fixtures";
import { RecordDeploymentDialog } from "./RecordDeploymentDialog";

const review = reviewConsumerDeployment(consumerRegisteredFixture, "beta", consumerFixture);

describe("manual deployment approval", () => {
  it("requires a reference and explicit approval, resetting approval if the reference changes", async () => {
    const confirm = vi.fn(async (_request: RecordConsumerDeployment) => consumerDeploymentFixture);
    const recorded = vi.fn();
    const user = userEvent.setup();
    render(<RecordDeploymentDialog review={review} onConfirm={confirm} onRecorded={recorded} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "deployed to / Personal MacBook" });
    const reference = within(dialog).getByRole("textbox", { name: "Release reference" });
    const approve = within(dialog).getByRole("checkbox");
    const submit = within(dialog).getByRole("button", { name: "approve & record" });
    expect(reference).toHaveFocus();
    expect(submit).toBeDisabled();
    await user.type(reference, "v1.5.0");
    expect(submit).toBeDisabled();
    await user.click(approve);
    expect(submit).toBeEnabled();
    await user.type(reference, "+8f1c2a9");
    expect(approve).not.toBeChecked();
    expect(submit).toBeDisabled();
    await user.type(within(dialog).getByRole("textbox", { name: /Note/ }), "Copied over USB.");
    await user.click(approve);
    await user.click(submit);
    expect(confirm).toHaveBeenCalledExactlyOnceWith({
      appId: "atlas", stageId: "beta", consumerId: "personal-mac",
      expectedManifestHash: review.manifestHash, expectedUpdatedAt: review.updatedAt,
      releaseRef: "v1.5.0+8f1c2a9", notes: "Copied over USB.", approved: true,
    });
    expect(recorded).toHaveBeenCalledWith(consumerDeploymentFixture);
  });

  it("cancels without creating a marker", async () => {
    const confirm = vi.fn();
    const close = vi.fn();
    const user = userEvent.setup();
    render(<RecordDeploymentDialog review={review} onConfirm={confirm} onRecorded={vi.fn()} onClose={close} />);
    await user.click(screen.getByRole("button", { name: "cancel" }));
    expect(close).toHaveBeenCalledOnce();
    expect(confirm).not.toHaveBeenCalled();
  });

  it("enforces Unicode character limits and rejects controls without rejecting valid notes", async () => {
    const confirm = vi.fn(async (_request: RecordConsumerDeployment) => consumerDeploymentFixture);
    const user = userEvent.setup();
    render(<RecordDeploymentDialog review={review} onConfirm={confirm} onRecorded={vi.fn()} onClose={vi.fn()} />);
    const reference = screen.getByRole("textbox", { name: "Release reference" });
    const notes = screen.getByRole("textbox", { name: /Note/ });
    const submit = screen.getByRole("button", { name: "approve & record" });
    fireEvent.change(reference, { target: { value: "\u{1f4e6}".repeat(256) } });
    await user.click(screen.getByRole("checkbox"));
    expect(submit).toBeEnabled();
    fireEvent.change(reference, { target: { value: `${"v".repeat(256)}\u{1f4e6}` } });
    expect(reference).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("Use at most 256 characters for the release reference.")).toBeVisible();
    expect(submit).toBeDisabled();
    fireEvent.change(reference, { target: { value: "v1.5.0" } });
    fireEvent.change(notes, { target: { value: "\u{1f4e6}".repeat(4096) } });
    await user.click(screen.getByRole("checkbox"));
    expect(notes).toHaveAttribute("aria-invalid", "false");
    expect(submit).toBeEnabled();
    fireEvent.change(notes, { target: { value: "x".repeat(4097) } });
    expect(notes).toHaveAttribute("aria-invalid", "true");
    expect(submit).toBeDisabled();
    fireEvent.change(notes, { target: { value: "First line\nSecond line\twith a tab" } });
    expect(submit).toBeEnabled();
    fireEvent.change(notes, { target: { value: "Bad\u001bcontrol" } });
    expect(notes).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("Notes may contain newlines and tabs, but no other control characters.")).toBeVisible();
    expect(submit).toBeDisabled();
    expect(confirm).not.toHaveBeenCalled();
  });

  it.each(["registration_changed", "consumer_not_found", "not_found"])("requires a fresh review after %s", async (code) => {
    const confirm = vi.fn(async (_request: RecordConsumerDeployment): Promise<ConsumerDeployment> => {
      throw { code, message: "The reviewed destination changed." };
    });
    const invalidated = vi.fn();
    const recorded = vi.fn();
    const user = userEvent.setup();
    render(<RecordDeploymentDialog review={review} initialReleaseRef="commit:8f1c2a9" onConfirm={confirm} onRecorded={recorded} onClose={vi.fn()} onInvalidated={invalidated} />);
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "approve & record" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The reviewed destination changed");
    expect(screen.getByRole("button", { name: "approve & record" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "Release reference" })).toBeDisabled();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    const form = screen.getByRole("textbox", { name: "Release reference" }).closest("form");
    if (!form) throw new Error("Approval form missing.");
    fireEvent.submit(form);
    expect(confirm).toHaveBeenCalledOnce();
    expect(invalidated).toHaveBeenCalledOnce();
    expect(recorded).not.toHaveBeenCalled();
  });

  it("keeps storage failures visible without reporting success", async () => {
    const confirm = vi.fn(async (_request: RecordConsumerDeployment): Promise<ConsumerDeployment> => {
      throw { code: "storage", message: "Database is locked." };
    });
    const recorded = vi.fn();
    const user = userEvent.setup();
    render(<RecordDeploymentDialog review={review} initialReleaseRef="sha256:abc123" onConfirm={confirm} onRecorded={recorded} onClose={vi.fn()} />);
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "approve & record" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Database is locked");
    expect(recorded).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "Release reference" })).toHaveValue("sha256:abc123");
    expect(screen.getByRole("button", { name: "approve & record" })).toBeEnabled();
  });

  it("prevents duplicate submissions and cancellation while the write is pending", async () => {
    let finish: ((record: ConsumerDeployment) => void) | undefined;
    const response = new Promise<ConsumerDeployment>((resolve) => { finish = resolve; });
    const confirm = vi.fn(async (_request: RecordConsumerDeployment) => response);
    const close = vi.fn();
    const recorded = vi.fn();
    const user = userEvent.setup();
    render(<RecordDeploymentDialog review={review} initialReleaseRef="v1.5.0" onConfirm={confirm} onRecorded={recorded} onClose={close} />);
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "approve & record" }));
    const dialog = screen.getByRole("dialog");
    const form = dialog.querySelector("form");
    if (!form || !finish) throw new Error("Pending approval fixture missing.");
    fireEvent.submit(form);
    fireEvent(dialog, new Event("cancel", { cancelable: true }));
    expect(screen.getByRole("button", { name: "recording..." })).toBeDisabled();
    expect(screen.getByRole("button", { name: "cancel" })).toBeDisabled();
    expect(confirm).toHaveBeenCalledOnce();
    expect(close).not.toHaveBeenCalled();
    const complete = finish;
    await act(async () => { complete(consumerDeploymentFixture); await response; });
    expect(recorded).toHaveBeenCalledOnce();
  });
});
