import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { reviewPromotion } from "../lib/promotion";
import { simulatePromotionAssessment } from "../lib/promotionPreview";
import type { ApprovePromotion, PromotionApproval, RecordHarnessResult } from "../lib/types";
import { consumerDeploymentFixture, harnessResultFixture, promotionRegisteredFixture } from "../test/fixtures";
import { ApprovePromotionDialog, ReportHarnessDialog } from "./PromotionDialogs";

const stage = promotionRegisteredFixture.manifest.stages.find((item) => item.id === "beta");
if (!stage) throw new Error("Missing promotion stage.");
const review = reviewPromotion(promotionRegisteredFixture, stage);
const ready = simulatePromotionAssessment(review, "v1.5.0", [harnessResultFixture], []);

describe("reported harness results", () => {
  it("requires actual counts rather than filling the observed total from the manifest", async () => {
    const confirm = vi.fn(async (_request: RecordHarnessResult) => harnessResultFixture);
    const user = userEvent.setup();
    render(<ReportHarnessDialog review={review} checkId="unit" onConfirm={confirm} onRecorded={vi.fn()} onClose={vi.fn()} />);
    const total = screen.getByRole("spinbutton", { name: "Tests reported" });
    const passed = screen.getByRole("spinbutton", { name: "Tests passed" });
    expect(total).toHaveDisplayValue("");
    expect(passed).toHaveDisplayValue("");
    expect(screen.getByRole("button", { name: "record results" })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    await user.type(total, "49");
    await user.type(passed, "45");
    await user.click(screen.getByRole("button", { name: "record results" }));
    expect(confirm).toHaveBeenCalledExactlyOnceWith({
      appId: "atlas", stageId: "beta", checkId: "unit", releaseRef: "v1.5.0",
      total: 49, passed: 45, notes: null,
      expectedManifestHash: review.manifestHash, expectedUpdatedAt: review.updatedAt,
    });
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("accepts zero observed tests as a report but rejects impossible counts", async () => {
    const confirm = vi.fn(async (_request: RecordHarnessResult) => harnessResultFixture);
    const user = userEvent.setup();
    render(<ReportHarnessDialog review={review} checkId="unit" initialReleaseRef="v1.5.0" onConfirm={confirm} onRecorded={vi.fn()} onClose={vi.fn()} />);
    const total = screen.getByRole("spinbutton", { name: "Tests reported" });
    const passed = screen.getByRole("spinbutton", { name: "Tests passed" });
    fireEvent.change(total, { target: { value: "50" } });
    fireEvent.change(passed, { target: { value: "51" } });
    expect(screen.getByRole("button", { name: "record results" })).toBeDisabled();
    expect(screen.getByText("Passed tests cannot exceed the reported total.")).toBeVisible();
    fireEvent.change(total, { target: { value: "0" } });
    fireEvent.change(passed, { target: { value: "0" } });
    await user.click(screen.getByRole("button", { name: "record results" }));
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ total: 0, passed: 0 }));
  });
});

describe("promotion approval and skipped steps", () => {
  it("requires separate acknowledgement before recording an optional skip", async () => {
    const approval: PromotionApproval = { id: "1", assessment: ready, skippedOptional: ["personal-delivery"], notes: null, recordedAt: "2026-10-27T21:00:00Z" };
    const confirm = vi.fn(async (_request: ApprovePromotion) => approval);
    const recorded = vi.fn();
    const user = userEvent.setup();
    render(<ApprovePromotionDialog assessment={ready} onConfirm={confirm} onRecorded={recorded} onClose={vi.fn()} />);
    const submit = screen.getByRole("button", { name: "approve & record decision" });
    await user.click(screen.getByRole("checkbox", { name: /I approve this promotion-criteria decision/ }));
    expect(submit).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: /I acknowledge that these optional steps are skipped/ }));
    await user.click(submit);
    expect(confirm).toHaveBeenCalledExactlyOnceWith({
      appId: "atlas", stageId: "beta", releaseRef: "v1.5.0",
      expectedManifestHash: ready.manifestHash, expectedUpdatedAt: ready.registrationUpdatedAt,
      expectedAssessmentHash: ready.assessmentHash, approved: true, acknowledgeOptional: true, notes: null,
    });
    expect(recorded).toHaveBeenCalledWith(approval);
  });

  it("does not invent skipped steps when every criterion is satisfied", async () => {
    const satisfied = simulatePromotionAssessment(review, "v1.5.0", [harnessResultFixture], [consumerDeploymentFixture]);
    const confirm = vi.fn(async (_request: ApprovePromotion): Promise<PromotionApproval> => ({
      id: "1", assessment: satisfied, skippedOptional: [], notes: null, recordedAt: "2026-10-27T21:00:00Z",
    }));
    const user = userEvent.setup();
    render(<ApprovePromotionDialog assessment={satisfied} onConfirm={confirm} onRecorded={vi.fn()} onClose={vi.fn()} />);
    expect(screen.queryByRole("checkbox", { name: /I acknowledge/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "approve & record decision" }));
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ acknowledgeOptional: false }));
  });

  it("does not approve a blocked assessment even if submission is forced", () => {
    const blocked = simulatePromotionAssessment(review, "v1.5.0", [{ ...harnessResultFixture, passed: 44 }], []);
    const confirm = vi.fn();
    render(<ApprovePromotionDialog assessment={blocked} onConfirm={confirm} onRecorded={vi.fn()} onClose={vi.fn()} />);
    const form = screen.getByRole("dialog").querySelector("form");
    if (!form) throw new Error("Missing approval form.");
    fireEvent.submit(form);
    expect(confirm).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "approve & record decision" })).toBeDisabled();
    expect(screen.getByRole("alert")).toBeVisible();
  });

  it("invalidates both approvals when evidence changes before the server transaction", async () => {
    const confirm = vi.fn(async (_request: ApprovePromotion): Promise<PromotionApproval> => {
      throw { code: "assessment_changed", message: "New results arrived; assess again." };
    });
    const invalidate = vi.fn();
    const user = userEvent.setup();
    render(<ApprovePromotionDialog assessment={ready} onConfirm={confirm} onRecorded={vi.fn()} onClose={vi.fn()} onInvalidated={invalidate} />);
    await user.click(screen.getByRole("checkbox", { name: /I approve this/ }));
    await user.click(screen.getByRole("checkbox", { name: /I acknowledge/ }));
    await user.click(screen.getByRole("button", { name: "approve & record decision" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("New results arrived");
    expect(screen.getByRole("button", { name: "approve & record decision" })).toBeDisabled();
    for (const checkbox of screen.getAllByRole("checkbox")) {
      expect(checkbox).not.toBeChecked();
      expect(checkbox).toBeDisabled();
    }
    expect(invalidate).toHaveBeenCalledOnce();
  });
});
