import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import { consumerDeploymentFixture, harnessResultFixture, makeApi, promotionRegisteredFixture } from "./test/fixtures";
import type { RegisteredApp } from "./lib/types";

async function openPromotion(api: ReturnType<typeof makeApi>) {
  const user = userEvent.setup();
  render(<App api={api} />);
  await user.click(await screen.findByRole("button", { name: "beta: not run" }));
  await user.click(screen.getByRole("button", { name: "promotion criteria" }));
  return user;
}

async function report(user: ReturnType<typeof userEvent.setup>, total: number, passed: number) {
  await user.click(screen.getByRole("button", { name: "Report results for unit" }));
  const dialog = screen.getByRole("dialog", { name: "report harness / unit" });
  fireEvent.change(within(dialog).getByRole("textbox", { name: "Release reference" }), { target: { value: "v1.5.0" } });
  await user.type(within(dialog).getByRole("spinbutton", { name: "Tests reported" }), String(total));
  await user.type(within(dialog).getByRole("spinbutton", { name: "Tests passed" }), String(passed));
  await user.click(within(dialog).getByRole("button", { name: "record results" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
}

describe("promotion reporting and approval workflow", () => {
  it("enforces the manifest's n/m assertion, records acknowledged skips, and never advances a real stage", async () => {
    const api = makeApi([promotionRegisteredFixture]);
    const user = await openPromotion(api);
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "v1.5.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("blocked");
    expect(screen.getByRole("button", { name: "review promotion approval" })).toBeDisabled();
    expect(api.approvePromotion).not.toHaveBeenCalled();

    await report(user, 49, 45);
    expect(api.recordHarnessResult).toHaveBeenLastCalledWith(expect.objectContaining({ total: 49, passed: 45, checkId: "unit", releaseRef: "v1.5.0" }));
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("Expected exactly 50");
    expect(screen.getByRole("button", { name: "review promotion approval" })).toBeDisabled();

    await report(user, 50, 45);
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("eligible with warnings");
    await user.click(screen.getByRole("button", { name: "review optional skips & approve" }));
    const dialog = screen.getByRole("dialog", { name: "approve promotion / beta" });
    await user.click(within(dialog).getByRole("checkbox", { name: /I approve this/ }));
    expect(within(dialog).getByRole("button", { name: "approve & record decision" })).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox", { name: /I acknowledge/ }));
    await user.click(within(dialog).getByRole("button", { name: "approve & record decision" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(api.approvePromotion).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
      appId: "atlas", stageId: "beta", releaseRef: "v1.5.0", approved: true, acknowledgeOptional: true,
      expectedManifestHash: promotionRegisteredFixture.manifest.manifestHash,
      expectedUpdatedAt: promotionRegisteredFixture.updatedAt,
      expectedAssessmentHash: expect.any(String),
    }));
    expect(screen.getByRole("list", { name: "Promotion approval history" })).toHaveTextContent("personal-delivery");
    expect(screen.getByRole("list", { name: "Promotion approval history" })).toHaveTextContent("approved with optional skips");
    for (const stage of ["test", "beta", "gamma", "prod"]) expect(screen.getByRole("button", { name: `${stage}: not run` })).toBeVisible();
    expect(screen.getByRole("button", { name: "run stage" })).toBeDisabled();
    expect(api.registerManifest).not.toHaveBeenCalled();
  });

  it("blocks a required target until the same candidate has a matching manual deployment", async () => {
    const app: RegisteredApp = {
      ...promotionRegisteredFixture,
      manifest: {
        ...promotionRegisteredFixture.manifest,
        stages: promotionRegisteredFixture.manifest.stages.map((stage) => ({
          ...stage,
          promotionCriteria: stage.promotionCriteria.map((criterion) => criterion.kind === "deployment"
            ? { ...criterion, requirement: "required" } : criterion),
        })),
      },
    };
    const api = makeApi([app], true, app.manifest, [], { harnessResults: [harnessResultFixture] });
    const user = await openPromotion(api);
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "v1.5.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("Required deployment");
    expect(screen.getByRole("button", { name: "review promotion approval" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "deployed to Personal MacBook" }));
    const dialog = screen.getByRole("dialog", { name: "deployed to / Personal MacBook" });
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "approve & record" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("criteria satisfied");
    expect(screen.getByRole("button", { name: "review promotion approval" })).toBeEnabled();
    expect(api.approvePromotion).not.toHaveBeenCalled();
  });

  it("retains a skipped optional step in approval history even after a later delivery", async () => {
    const api = makeApi([promotionRegisteredFixture], true, promotionRegisteredFixture.manifest, [], { harnessResults: [harnessResultFixture] });
    const user = await openPromotion(api);
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "v1.5.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    await user.click(await screen.findByRole("button", { name: "review optional skips & approve" }));
    const approvalDialog = screen.getByRole("dialog");
    for (const checkbox of within(approvalDialog).getAllByRole("checkbox")) await user.click(checkbox);
    await user.click(within(approvalDialog).getByRole("button", { name: "approve & record decision" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: "deployed to Personal MacBook" }));
    const deploymentDialog = screen.getByRole("dialog");
    await user.type(within(deploymentDialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
    await user.click(within(deploymentDialog).getByRole("checkbox"));
    await user.click(within(deploymentDialog).getByRole("button", { name: "approve & record" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("criteria satisfied");
    expect(screen.getByRole("list", { name: "Promotion approval history" })).toHaveTextContent("approved with optional skips");
    await user.click(screen.getByRole("button", { name: "unregister" }));
    await user.click(within(screen.getByRole("dialog", { name: "unregister / atlas" })).getByRole("button", { name: "unregister" }));
    await screen.findByRole("heading", { name: "No manifests yet." });
    await user.click(screen.getByRole("button", { name: "Releases" }));
    expect(screen.getByRole("region", { name: "Recorded promotion approvals" })).toHaveTextContent("personal-delivery");
    expect(screen.getByRole("region", { name: "Recorded harness results" })).toHaveTextContent("45/50 reported passes");
  });

  it("keeps a stale approval open with an explicit re-assessment error", async () => {
    const api = makeApi([promotionRegisteredFixture], true, promotionRegisteredFixture.manifest, [consumerDeploymentFixture], { harnessResults: [harnessResultFixture] });
    const user = await openPromotion(api);
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "v1.5.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    await user.click(await screen.findByRole("button", { name: "review promotion approval" }));
    api.approvePromotion.mockRejectedValueOnce({ code: "assessment_changed", message: "New failing results arrived. Assess again." });
    api.listHarnessResults.mockResolvedValueOnce([{ ...harnessResultFixture, id: "2", passed: 44 }]);
    const dialog = screen.getByRole("dialog");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "approve & record decision" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("New failing results");
    expect(within(dialog).getByRole("button", { name: "approve & record decision" })).toBeDisabled();
    expect(api.approvePromotion).toHaveBeenCalledOnce();
    expect(screen.queryByRole("list", { name: "Promotion approval history" })).not.toBeInTheDocument();
  });

  it("does not turn a backend assessment failure into readiness", async () => {
    const api = makeApi([promotionRegisteredFixture]);
    api.assessPromotion.mockRejectedValueOnce({ code: "storage", message: "Evidence could not be read." });
    const user = await openPromotion(api);
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "v1.5.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Evidence could not be read");
    expect(screen.queryByRole("region", { name: "Promotion readiness assessment" })).not.toBeInTheDocument();
    expect(api.approvePromotion).not.toHaveBeenCalled();
  });
});
