import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { reviewPromotion } from "../lib/promotion";
import { simulatePromotionAssessment } from "../lib/promotionPreview";
import { harnessResultFixture, promotionRegisteredFixture } from "../test/fixtures";
import type { PromotionAssessment, PromotionContext } from "../lib/types";
import { PromotionApprovalHistory, PromotionPanel } from "./PromotionCriteria";

const stage = promotionRegisteredFixture.manifest.stages.find((item) => item.id === "beta");
if (!stage) throw new Error("Missing promotion stage fixture.");
const review = reviewPromotion(promotionRegisteredFixture, stage);
const ready = simulatePromotionAssessment(review, "v1.5.0", [harnessResultFixture], []);

describe("promotion criteria UI", () => {
  it("shows exact manifest thresholds without assuming that declared checks passed", () => {
    render(<PromotionPanel stage={stage} />);
    expect(screen.getByText("45")).toBeVisible();
    expect(screen.getByText("50")).toBeVisible();
    expect(screen.getByText("required harness")).toBeVisible();
    expect(screen.getByText("optional deployment")).toBeVisible();
    expect(screen.queryByRole("region", { name: "Promotion readiness assessment" })).not.toBeInTheDocument();
    expect(screen.getByText(/Register this manifest to report results/)).toBeVisible();
    expect(screen.getByText("other stage requirements / not evaluated")).toBeVisible();
  });

  it("asks the service for candidate-specific readiness and makes optional skips explicit", async () => {
    const assess = vi.fn(async (_context: PromotionContext) => ready);
    const approve = vi.fn();
    const user = userEvent.setup();
    render(<PromotionPanel stage={stage} review={review} results={[harnessResultFixture]} onAssess={assess} onApprove={approve} />);
    expect(assess).not.toHaveBeenCalled();
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "v1.5.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(assess).toHaveBeenCalledExactlyOnceWith({
      appId: "atlas", stageId: "beta", releaseRef: "v1.5.0",
      expectedManifestHash: review.manifestHash, expectedUpdatedAt: review.updatedAt,
    });
    const assessment = await screen.findByRole("region", { name: "Promotion readiness assessment" });
    expect(assessment).toHaveTextContent("eligible with warnings");
    expect(assessment).toHaveTextContent("45/50");
    expect(assessment).toHaveTextContent("personal-delivery");
    await user.click(screen.getByRole("button", { name: "review optional skips & approve" }));
    expect(approve).toHaveBeenCalledWith(ready);
  });

  it("does not allow approval when a required criterion is blocked", async () => {
    const blocked = simulatePromotionAssessment(review, "v1.5.0", [{ ...harnessResultFixture, total: 49 }], []);
    const user = userEvent.setup();
    render(<PromotionPanel stage={stage} review={review} initialReleaseRef="v1.5.0" onAssess={async () => blocked} onApprove={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("49");
    expect(screen.getByRole("button", { name: "review promotion approval" })).toBeDisabled();
  });

  it("discards an in-flight assessment when relevant evidence changes", async () => {
    let finish: ((assessment: PromotionAssessment) => void) | undefined;
    const pending = new Promise<PromotionAssessment>((resolve) => { finish = resolve; });
    const assess = vi.fn(async () => pending);
    const user = userEvent.setup();
    const { rerender } = render(<PromotionPanel stage={stage} review={review} initialReleaseRef="v1.5.0" results={[harnessResultFixture]} onAssess={assess} onApprove={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    rerender(<PromotionPanel stage={stage} review={review} initialReleaseRef="v1.5.0" results={[{ ...harnessResultFixture, id: "2", passed: 44 }]} onAssess={assess} onApprove={vi.fn()} />);
    if (!finish) throw new Error("Missing assessment promise.");
    const resolve = finish;
    await act(async () => { resolve(ready); await pending; });
    expect(screen.queryByRole("region", { name: "Promotion readiness assessment" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /review optional skips/ })).not.toBeInTheDocument();
  });

  it("keeps an unrelated candidate's reports from invalidating the current assessment", async () => {
    const user = userEvent.setup();
    const assess = vi.fn(async () => ready);
    const { rerender } = render(<PromotionPanel stage={stage} review={review} initialReleaseRef="v1.5.0" results={[harnessResultFixture]} onAssess={assess} />);
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    await screen.findByRole("region", { name: "Promotion readiness assessment" });
    rerender(<PromotionPanel stage={stage} review={review} initialReleaseRef="v1.5.0" results={[{ ...harnessResultFixture, id: "2", releaseRef: "v9" }, harnessResultFixture]} onAssess={assess} />);
    expect(screen.getByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("eligible with warnings");
  });

  it("retains past optional skips as a frozen decision rather than a current warning", async () => {
    const user = userEvent.setup();
    render(<PromotionApprovalHistory approvals={[{
      id: "1", assessment: ready, skippedOptional: ["personal-delivery"], notes: null, recordedAt: "2026-10-27T21:00:00Z",
    }]} />);
    expect(screen.getByText("approved with optional skips")).toBeVisible();
    await user.click(screen.getByText("approval #1 / frozen decision"));
    expect(screen.getByRole("region", { name: "Recorded promotion assessment" })).toHaveTextContent("were acknowledged as skipped");
    expect(screen.getByText(/Original criteria, evidence, and skipped warnings are retained/)).toBeVisible();
  });
});
