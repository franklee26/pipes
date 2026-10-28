import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import { makeApi, makeLog, makeRun, manifestFixture, promotionRegisteredFixture, registeredFixture } from "./test/fixtures";
import type { PipelineRun } from "./lib/types";

function finishedRun(): PipelineRun {
  const run = makeRun({ status: "passed", finishedAt: "2026-10-27T22:02:00Z" });
  run.stages = run.stages.map((stage) => ({ ...stage, status: "passed" }));
  run.commands = run.commands.map((command) => ({ ...command, status: "passed", exitCode: 0 }));
  return run;
}

async function inspect(api: ReturnType<typeof makeApi>, runId = "run-1") {
  const user = userEvent.setup();
  const rendered = render(<App api={api} />);
  await screen.findByRole("heading", { name: /pipelines/ });
  await user.click(screen.getByRole("button", { name: "Releases" }));
  await user.click(await screen.findByRole("button", { name: `inspect run ${runId}` }));
  await screen.findByRole("region", { name: "Pipeline terminal output" });
  return { user, ...rendered, detail: screen.getByRole("region", { name: "Pipeline run details" }) };
}

describe("real pipeline run workspace", () => {
  it("starts only after explicit confirmation, opens live progress and cancels the real run", async () => {
    const api = makeApi([registeredFixture]);
    const user = userEvent.setup();
    const { container } = render(<App api={api} />);
    await screen.findByRole("button", { name: "Atlas" });
    await user.click(screen.getByRole("button", { name: "run pipeline" }));
    const dialog = screen.getByRole("dialog", { name: "review execution / Atlas" });
    expect(api.startPipelineRun).not.toHaveBeenCalled();
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "candidate-42");
    expect(within(dialog).getByRole("button", { name: "approve & start run" })).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "approve & start run" }));
    const detail = await screen.findByRole("region", { name: "Pipeline run details" });
    expect(await within(detail).findByRole("heading", { name: "Atlas / candidate-42" })).toBeVisible();
    expect(api.startPipelineRun).toHaveBeenCalledExactlyOnceWith({
      appId: "atlas", expectedManifestHash: manifestFixture.manifestHash,
      expectedUpdatedAt: registeredFixture.updatedAt, releaseRef: "candidate-42", approved: true,
    });
    expect(api.getPipelineRun).toHaveBeenCalledWith("run-1");
    expect(container.querySelector('[data-preview="true"]')).toBeNull();
    await user.click(within(detail).getByRole("button", { name: "cancel run" }));
    await waitFor(() => expect(api.cancelPipelineRun).toHaveBeenCalledExactlyOnceWith("run-1"));
    expect(await within(detail).findByText("Cancelled by user.")).toBeVisible();
    expect(within(detail).queryByRole("button", { name: "cancel run" })).not.toBeInTheDocument();
    await user.click(within(detail).getByRole("button", { name: "run history" }));
    expect(await screen.findByRole("region", { name: "Pipeline run history" })).toHaveTextContent("candidate-42");
  });

  it("shows a local stage gate before its commands and requires per-run explicit approval", async () => {
    const run = makeRun({ status: "waiting_approval" });
    run.stages = run.stages.map((stage) => ({ ...stage, status: stage.stageId === "gamma" ? "waiting_approval" : stage.stageId === "prod" ? "pending" : "passed" }));
    const api = makeApi([registeredFixture], true, manifestFixture, [], { runs: [run] });
    const { user, detail } = await inspect(api);
    const stage = within(detail).getByRole("region", { name: "gamma run status" });
    expect(stage).toHaveTextContent("waiting BEFORE its commands");
    expect(within(stage).getByRole("button", { name: "approve stage gamma" })).toBeDisabled();
    expect(api.approvePipelineStage).not.toHaveBeenCalled();
    await user.click(within(stage).getByRole("checkbox", { name: "I approve executing stage gamma for this run." }));
    await user.click(within(stage).getByRole("button", { name: "approve stage gamma" }));
    await waitFor(() => expect(api.approvePipelineStage).toHaveBeenCalledExactlyOnceWith(run.id, "gamma", true));
    await waitFor(() => expect(within(detail).queryByRole("button", { name: "approve stage gamma" })).not.toBeInTheDocument());
    expect(stage).toHaveTextContent("2026-10-27T22:01:00Z");
  });

  it("surfaces approval errors without locally advancing the stage", async () => {
    const run = makeRun({ status: "waiting_approval" });
    run.stages[2]!.status = "waiting_approval";
    const api = makeApi([registeredFixture], true, manifestFixture, [], { runs: [run] });
    api.approvePipelineStage.mockRejectedValueOnce({ code: "run_conflict", message: "The approval is no longer valid." });
    const { user, detail } = await inspect(api);
    await user.click(within(detail).getByRole("checkbox", { name: "I approve executing stage gamma for this run." }));
    await user.click(within(detail).getByRole("button", { name: "approve stage gamma" }));
    expect(await within(detail).findByRole("alert")).toHaveTextContent("The approval is no longer valid");
    expect(within(detail).getByRole("region", { name: "gamma run status" })).toHaveTextContent("waiting approval");
    expect(within(detail).getByRole("checkbox", { name: "I approve executing stage gamma for this run." })).not.toBeChecked();
  });

  it("loads persisted history and frozen output after reload even without a registered app", async () => {
    const run = finishedRun();
    const api = makeApi([], true, manifestFixture, [], { runs: [run], logs: { [run.id]: [makeLog("saved output\n")] } });
    const first = await inspect(api);
    expect(first.detail).toHaveTextContent("App unregistered. History and output are retained.");
    expect(await within(first.detail).findByText("saved output")).toBeVisible();
    first.unmount();
    const second = await inspect(api);
    expect(await within(second.detail).findByText("saved output")).toBeVisible();
    expect(api.listPipelineRuns).toHaveBeenCalledTimes(2);
    expect(api.startPipelineRun).not.toHaveBeenCalled();
  });

  it.each(["pending", "running"] as const)("keeps a %s preflight run inspectable and cancellable after unregistration", async (status) => {
    const run = makeRun({ status, message: "Git cleanliness preflight is running." });
    const api = makeApi([registeredFixture], true, manifestFixture, [], { runs: [run], logs: {
      [run.id]: [makeLog("Git preflight: checking source cleanliness.\n", 1, { stageId: null, commandId: null, stream: "system" })],
    } });
    const user = userEvent.setup();
    render(<App api={api} />);
    await user.click(await screen.findByRole("button", { name: "Atlas" }));
    await user.click(screen.getByRole("button", { name: "unregister" }));
    const dialog = screen.getByRole("dialog", { name: "unregister / atlas" });
    expect(dialog).toHaveTextContent("Pipeline runs, frozen manifests, and terminal output remain in Releases");
    expect(dialog).toHaveTextContent("Removing a registration does not cancel an active run");
    await user.click(within(dialog).getByRole("button", { name: "unregister" }));
    await screen.findByRole("heading", { name: "No manifests yet." });
    await user.click(screen.getByRole("button", { name: "Releases" }));
    expect(screen.getByRole("region", { name: "Pipeline run history" })).toHaveTextContent("app unregistered");
    await user.click(screen.getByRole("button", { name: "inspect run run-1" }));
    expect(await screen.findByText("Git preflight: checking source cleanliness.")).toBeVisible();
    expect(api.cancelPipelineRun).not.toHaveBeenCalled();
    const detail = screen.getByRole("region", { name: "Pipeline run details" });
    expect(detail).toHaveTextContent(run.message!);
    expect(within(screen.getByRole("region", { name: "Recorded command outcomes" })).getByText("pending")).toBeVisible();
    expect(detail).toHaveTextContent("App unregistered. History and output are retained.");
    await user.click(within(detail).getByRole("button", { name: "cancel run" }));
    expect(await within(detail).findByText("Cancelled by user.")).toBeVisible();
    expect(api.cancelPipelineRun).toHaveBeenCalledExactlyOnceWith(run.id);
    expect(api.startPipelineRun).not.toHaveBeenCalled();
  });

  it("keeps historic telemetry separate from a changed current registration, even with the same hash", async () => {
    const current = structuredClone(registeredFixture);
    current.updatedAt = "2026-10-27T23:00:00Z";
    current.manifest.sourcePath = "/new/source";
    const run = finishedRun();
    const api = makeApi([current], true, current.manifest, [], { runs: [run] });
    const { user, detail, container } = await inspect(api);
    expect(detail).toHaveTextContent("Different from the current registration");
    expect(detail).toHaveTextContent("/projects/atlas");
    expect(container.querySelector('[data-preview="true"]')).toBeNull();
    await user.click(within(detail).getByRole("button", { name: "view current configuration" }));
    expect(await screen.findByRole("button", { name: "test: not run" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "Recorded stage progress" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Pipeline run history" })).toHaveTextContent("different registration / frozen snapshot");
  });

  it("displays actual failure details, timeout and blocked outcomes without manufacturing test counts", async () => {
    const run = makeRun({ status: "timed_out", finishedAt: "2026-10-27T22:00:05Z", message: "Pipeline timeout after 5 seconds." });
    run.commands[0] = { ...run.commands[0]!, status: "timed_out", startedAt: run.startedAt, finishedAt: run.finishedAt, message: "Command exceeded its deadline." };
    run.stages = run.stages.map((stage) => ({ ...stage, status: stage.stageId === "test" ? "timed_out" : "blocked", message: "All parent stages must pass." }));
    const api = makeApi([], true, manifestFixture, [], { runs: [run], logs: { [run.id]: [makeLog("deadline reached", 1, { stream: "stderr" })] } });
    const { detail } = await inspect(api);
    expect(detail).toHaveTextContent("Pipeline timeout after 5 seconds.");
    expect(detail).toHaveTextContent("Command exceeded its deadline.");
    expect(detail).toHaveTextContent("0/1 commands passed");
    expect(await within(detail).findByText("deadline reached")).toBeVisible();
    expect(within(detail).getByRole("button", { name: "beta: blocked" })).toBeVisible();
    expect(detail).not.toHaveTextContent("tests passed");
  });

  it.each([false, true])("routes a custom promotion policy wait separately (local approval required: %s)", async (requiresApproval) => {
    const registration = structuredClone(promotionRegisteredFixture);
    registration.manifest.stages[1]!.requiresApproval = requiresApproval;
    const run = makeRun({ manifest: registration.manifest, status: "waiting_approval" });
    run.stages[0]!.status = "passed";
    run.stages[1]!.status = "waiting_approval";
    run.stages[1]!.approvedAt = requiresApproval ? "2026-10-27T22:01:00Z" : null;
    run.stages[1]!.message = "Promotion criteria ready; explicitly approve this candidate.";
    run.commands = run.commands.map((command) => ({ ...command, status: "passed", exitCode: 0 }));
    const api = makeApi([registration], true, registration.manifest, [], { runs: [run] });
    const { user, detail } = await inspect(api);
    expect(detail).toHaveTextContent("Waiting for custom promotion policy");
    expect(within(detail).queryByRole("button", { name: "approve stage beta" })).not.toBeInTheDocument();
    await user.click(within(detail).getByRole("button", { name: "review promotion criteria" }));
    expect(await screen.findByRole("tabpanel", { name: "promotion" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Candidate release reference" })).toHaveValue(run.releaseRef);
    expect(api.approvePipelineStage).not.toHaveBeenCalled();
    expect(api.approvePromotion).not.toHaveBeenCalled();
    expect(api.recordHarnessResult).not.toHaveBeenCalled();
  });

  it("waits for the runner after local approval before offering the post-command policy workflow", async () => {
    const registration = structuredClone(promotionRegisteredFixture);
    registration.manifest.stages[1]!.requiresApproval = true;
    const run = makeRun({ manifest: registration.manifest, releaseRef: "candidate-from-frozen-run", status: "waiting_approval" });
    run.stages[0]!.status = "passed";
    run.stages[1]!.status = "waiting_approval";
    run.stages[1]!.message = "Local execution approval is required before this stage's commands run.";
    const accepted = structuredClone(run);
    accepted.stages[1]!.approvedAt = "2026-10-27T22:01:00Z";
    const api = makeApi([registration], true, registration.manifest, [], { runs: [run] });
    api.approvePipelineStage.mockImplementationOnce(async () => {
      api.getPipelineRun.mockResolvedValue(accepted);
      return accepted;
    });
    const { user, detail } = await inspect(api);
    const inspector = within(detail).getByRole("region", { name: "beta run status" });
    expect(inspector).toHaveTextContent(run.stages[1]!.message!);
    await user.click(within(detail).getByRole("checkbox", { name: "I approve executing stage beta for this run." }));
    await user.click(within(detail).getByRole("button", { name: "approve stage beta" }));
    expect(await within(detail).findByRole("heading", { name: "Local approval recorded" })).toBeVisible();
    expect(inspector).not.toHaveTextContent(run.stages[1]!.message!);
    expect(within(detail).queryByRole("button", { name: "approve stage beta" })).not.toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: "review promotion criteria" })).not.toBeInTheDocument();
    const policyWait = structuredClone(accepted);
    policyWait.stages[1]!.startedAt = "2026-10-27T22:01:01Z";
    policyWait.stages[1]!.message = "Promotion criteria require current external evidence and explicit policy approval.";
    policyWait.commands = policyWait.commands.map((command) => ({ ...command, status: "passed", exitCode: 0 }));
    api.getPipelineRun.mockResolvedValue(policyWait);
    expect(await within(detail).findByText(policyWait.stages[1]!.message!)).toBeVisible();
    await user.click(await within(detail).findByRole("button", { name: "review promotion criteria" }));
    expect(screen.getByRole("textbox", { name: "Candidate release reference" })).toHaveValue(run.releaseRef);
    expect(api.approvePipelineStage).toHaveBeenCalledExactlyOnceWith(run.id, "beta", true);
    expect(api.approvePromotion).not.toHaveBeenCalled();
    expect(api.recordHarnessResult).not.toHaveBeenCalled();
  });

  it("loads an app's older runs even when 100 newer runs from other apps fill the global history", async () => {
    const otherManifest = { ...manifestFixture, appId: "other", name: "Other" };
    const recent = Array.from({ length: 100 }, (_, index) => makeRun({ id: `other-${index}`, manifest: otherManifest, status: "passed" }));
    const older = makeRun({ id: "older-atlas", releaseRef: "v0.1.0", status: "passed", startedAt: "2026-10-25T00:00:00Z" });
    const api = makeApi([registeredFixture], true, manifestFixture, [], { runs: [...recent, older] });
    const user = userEvent.setup();
    render(<App api={api} />);
    await user.click(await screen.findByRole("button", { name: "Atlas" }));
    await waitFor(() => expect(api.listPipelineRuns).toHaveBeenCalledWith("atlas"));
    const history = screen.getByRole("region", { name: "Pipeline run history" });
    expect(await within(history).findByRole("button", { name: "inspect run older-atlas" })).toBeVisible();
    expect(history).toHaveTextContent("Latest 100 runs in this scope");
    await user.click(within(history).getByRole("button", { name: "inspect run older-atlas" }));
    expect(await screen.findByRole("heading", { name: "Atlas / v0.1.0" })).toBeVisible();
  });
  it("retains registration previews and ConceptPreview as completely nonexecuting", async () => {
    const api = makeApi([registeredFixture]);
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("button", { name: "Atlas" });
    await user.click(screen.getByRole("button", { name: "design preview" }));
    await user.click(screen.getByRole("button", { name: "run pipeline" }));
    expect(api.startPipelineRun).not.toHaveBeenCalled();
    expect(api.cancelPipelineRun).not.toHaveBeenCalled();
    expect(api.approvePipelineStage).not.toHaveBeenCalled();
  });
});
