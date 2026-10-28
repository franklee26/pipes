import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import { composedManifestFixture, consumerDeploymentFixture, consumerFixture, consumerRegisteredFixture, forkManifestFixture, makeApi, manifestFixture, registeredFixture } from "./test/fixtures";

async function startRegistration(api: ReturnType<typeof makeApi>) {
  const user = userEvent.setup();
  render(<App api={api} />);
  await waitFor(() => expect(api.listApps).toHaveBeenCalled());
  await user.click(screen.getByRole("button", { name: "manifest" }));
  const dialog = screen.getByRole("dialog", { name: "register manifest" });
  expect(within(dialog).getByRole("textbox", { name: "Manifest path" })).toHaveFocus();
  await user.type(within(dialog).getByRole("textbox", { name: "Manifest path" }), manifestFixture.manifestPath);
  await user.click(within(dialog).getByRole("button", { name: "review" }));
  return { user, dialog };
}

describe("Pipes desktop foundation", () => {
  it("starts empty without inventing successful runs", async () => {
    render(<App api={makeApi()} />);
    expect(await screen.findByRole("heading", { name: "No manifests yet." })).toBeVisible();
    expect(screen.getByText("0 apps / 0 runs")).toBeVisible();
    expect(screen.queryByText("passed", { exact: true })).not.toBeInTheDocument();
  });

  async function openConsumerDialog(api: ReturnType<typeof makeApi>) {
    const user = userEvent.setup();
    render(<App api={api} />);
    await user.click(await screen.findByRole("button", { name: "beta: not run" }));
    await user.click(screen.getByRole("button", { name: "deployed to Personal MacBook" }));
    return { user, dialog: screen.getByRole("dialog", { name: "deployed to / Personal MacBook" }) };
  }

  describe("manual consumer deployment workflow", () => {
    it("records only after approval, shows the persistent marker, and leaves stages unrun", async () => {
      const api = makeApi([consumerRegisteredFixture]);
      const { user, dialog } = await openConsumerDialog(api);
      expect(api.recordConsumerDeployment).not.toHaveBeenCalled();
      expect(within(dialog).getByText(consumerFixture.target)).toBeVisible();
      await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
      await user.type(within(dialog).getByRole("textbox", { name: /Note/ }), "Installed manually.");
      expect(within(dialog).getByRole("button", { name: "approve & record" })).toBeDisabled();
      await user.click(within(dialog).getByRole("checkbox"));
      await user.click(within(dialog).getByRole("button", { name: "approve & record" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      expect(api.recordConsumerDeployment).toHaveBeenCalledExactlyOnceWith({
        appId: "atlas", stageId: "beta", consumerId: "personal-mac",
        expectedManifestHash: consumerRegisteredFixture.manifest.manifestHash,
        expectedUpdatedAt: consumerRegisteredFixture.updatedAt,
        releaseRef: "v1.5.0", notes: "Installed manually.", approved: true,
      });
      const history = await screen.findByRole("tabpanel", { name: "deployments (1)" });
      expect(history).toHaveTextContent("v1.5.0");
      expect(history).toHaveTextContent(consumerFixture.target);
      expect(screen.getByRole("tab", { name: "deployments (1)" })).toHaveAttribute("aria-selected", "true");
      const flow = screen.getByRole("list", { name: "Configured pipeline" });
      expect(within(flow).getAllByRole("button").map((node) => node.getAttribute("aria-label"))).toEqual([
        "test: not run", "beta: not run", "gamma: not run", "prod: not run",
      ]);
      expect(screen.getByRole("button", { name: "run stage" })).toBeDisabled();
      expect(api.registerManifest).not.toHaveBeenCalled();
      await user.click(screen.getByRole("button", { name: "refresh" }));
      expect(await screen.findByRole("tabpanel", { name: "deployments (1)" })).toHaveTextContent("v1.5.0");
    });

    it("retains original deployment history after unregistration", async () => {
      const api = makeApi([consumerRegisteredFixture], true, consumerRegisteredFixture.manifest, [consumerDeploymentFixture]);
      const user = userEvent.setup();
      render(<App api={api} />);
      await user.click(await screen.findByRole("button", { name: "Atlas" }));
      await user.click(screen.getByRole("button", { name: "unregister" }));
      const dialog = screen.getByRole("dialog", { name: "unregister / atlas" });
      expect(dialog).toHaveTextContent("Recorded consumer deployments are retained");
      await user.click(within(dialog).getByRole("button", { name: "unregister" }));
      await screen.findByRole("heading", { name: "No manifests yet." });
      await user.click(screen.getByRole("button", { name: "Releases" }));
      const history = screen.getByRole("region", { name: "Recorded consumer deployments" });
      expect(history).toHaveTextContent("v1.5.0");
      expect(history).toHaveTextContent(consumerFixture.target);
      expect(within(history).getByText("app unregistered")).toBeVisible();
      expect(api.recordConsumerDeployment).not.toHaveBeenCalled();
    });

    it("invalidates an open approval when its reviewed registration changes, even with the same hash", async () => {
      const api = makeApi([consumerRegisteredFixture]);
      const { user, dialog } = await openConsumerDialog(api);
      const relocated = structuredClone(consumerRegisteredFixture);
      relocated.updatedAt = "2026-10-27T19:00:00Z";
      relocated.manifest.manifestPath = "/relocated/pipes.yaml";
      relocated.manifest.sourcePath = "/relocated/source";
      api.recordConsumerDeployment.mockRejectedValueOnce({ code: "registration_changed", message: "Registration moved. Review it again." });
      api.listApps.mockResolvedValueOnce([relocated]);
      await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "commit:8f1c2a9");
      await user.click(within(dialog).getByRole("checkbox"));
      await user.click(within(dialog).getByRole("button", { name: "approve & record" }));
      expect(await within(dialog).findByRole("alert")).toHaveTextContent("Registration moved");
      expect(within(dialog).getByRole("button", { name: "approve & record" })).toBeDisabled();
      expect(api.recordConsumerDeployment).toHaveBeenCalledWith(expect.objectContaining({
        expectedManifestHash: relocated.manifest.manifestHash,
        expectedUpdatedAt: consumerRegisteredFixture.updatedAt,
      }));
      await user.click(within(dialog).getByRole("button", { name: "cancel" }));
      await waitFor(() => expect(screen.getByRole("button", { name: "deployed to Personal MacBook" })).toBeEnabled());
      await user.click(screen.getByRole("button", { name: "deployed to Personal MacBook" }));
      const freshReview = screen.getByRole("dialog");
      await user.click(within(freshReview).getByText("reviewed registration"));
      expect(within(freshReview).getByText(relocated.manifest.manifestPath)).toBeVisible();
      expect(within(freshReview).getByText(relocated.updatedAt)).toBeVisible();
      expect(within(freshReview).getByRole("checkbox")).not.toBeChecked();
      expect(api.recordConsumerDeployment).toHaveBeenCalledOnce();
    });

    it("restores keyboard focus when an app disappears during an approval", async () => {
      const api = makeApi([consumerRegisteredFixture]);
      const { user, dialog } = await openConsumerDialog(api);
      api.recordConsumerDeployment.mockRejectedValueOnce({ code: "not_found", message: "The app was unregistered." });
      api.listApps.mockResolvedValueOnce([]);
      await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), "v1.5.0");
      await user.click(within(dialog).getByRole("checkbox"));
      await user.click(within(dialog).getByRole("button", { name: "approve & record" }));
      await within(dialog).findByRole("alert");
      await screen.findByRole("heading", { name: "No manifests yet." });
      await user.click(within(dialog).getByRole("button", { name: "cancel" }));
      expect(screen.getByRole("heading", { name: "pipelines_", level: 1 })).toHaveFocus();
    });

    it("reports history read errors rather than pretending there are no records", async () => {
      const api = makeApi([consumerRegisteredFixture], true, consumerRegisteredFixture.manifest, [consumerDeploymentFixture]);
      api.listConsumerDeployments.mockRejectedValueOnce({ code: "storage", message: "Deployment history is unreadable." });
      const user = userEvent.setup();
      render(<App api={api} />);
      expect(await screen.findByRole("alert")).toHaveTextContent("Deployment history is unreadable");
      expect(screen.queryByText(/No deployments recorded/)).not.toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "refresh" }));
      await screen.findByRole("button", { name: "Atlas" });
      await user.click(screen.getByRole("button", { name: "Releases" }));
      expect(screen.getByRole("region", { name: "Recorded consumer deployments" })).toHaveTextContent("v1.5.0");
    });
  });

  it("renders registered stage order and the configured, unrun state", async () => {
    render(<App api={makeApi([registeredFixture])} />);
    const flow = await screen.findByRole("list", { name: "Atlas stages" });
    expect(within(flow).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual([
      "test: not run", "beta: not run", "gamma: not run", "prod: not run",
    ]);
    expect(flow.parentElement?.querySelectorAll(".stage-arrow")).toHaveLength(3);
    expect(within(flow).getAllByText("not run")).toHaveLength(4);
  });

  it("previews without registering, then sends the reviewed hash on explicit confirmation", async () => {
    const api = makeApi();
    const { user, dialog } = await startRegistration(api);
    expect(await within(dialog).findByText("valid manifest")).toBeVisible();
    expect(api.registerManifest).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "register" }));
    expect(await screen.findByText("Atlas registered. No commands ran.")).toBeVisible();
    expect(api.registerManifest).toHaveBeenCalledWith(manifestFixture.manifestPath, manifestFixture.manifestHash, false);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Atlas", level: 1 })).toBeVisible();
    await waitFor(() => expect(screen.getByRole("heading", { name: "Atlas", level: 1 })).toHaveFocus());
  });

  it("shows validation errors rather than a success-shaped empty preview", async () => {
    const api = makeApi();
    api.previewManifest.mockRejectedValueOnce({
      code: "invalid_manifest",
      message: "stages: duplicate stage id 'test'",
    });
    const { dialog } = await startRegistration(api);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("duplicate stage id");
    expect(within(dialog).queryByRole("button", { name: "register" })).not.toBeInTheDocument();
    expect(api.registerManifest).not.toHaveBeenCalled();
  });

  it("invalidates a preview when the input path changes", async () => {
    const api = makeApi();
    const { user, dialog } = await startRegistration(api);
    await within(dialog).findByText("valid manifest");
    await user.type(within(dialog).getByRole("textbox", { name: "Manifest path" }), ".changed");
    expect(within(dialog).queryByText("valid manifest")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "register" })).not.toBeInTheDocument();
  });

  it("requires another review if the backend detects file drift", async () => {
    const api = makeApi();
    api.registerManifest.mockRejectedValueOnce({
      code: "manifest_changed",
      message: "Manifest changed since preview. Review it again.",
    });
    const { user, dialog } = await startRegistration(api);
    await within(dialog).findByText("valid manifest");
    await user.click(within(dialog).getByRole("button", { name: "register" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Review it again");
    expect(within(dialog).queryByText("valid manifest")).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "review" })).toBeEnabled();
  });

  it("requires opt-in before replacing an existing snapshot", async () => {
    const api = makeApi([registeredFixture]);
    api.previewManifest.mockResolvedValueOnce({ ...manifestFixture, manifestHash: "b".repeat(64) });
    const { user, dialog } = await startRegistration(api);
    const replace = await within(dialog).findByRole("button", { name: "replace registration" });
    expect(replace).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox", { name: "Replace the existing registered snapshot." }));
    await user.click(replace);
    await waitFor(() => expect(api.registerManifest).toHaveBeenCalledWith(
      manifestFixture.manifestPath, "b".repeat(64), true,
    ));
  });

  it("requires replacement approval for a changed resolved source even when YAML and manifest path match", async () => {
    const relocatedSource = { ...manifestFixture, sourcePath: "/relocated/source" };
    const api = makeApi([registeredFixture], true, relocatedSource);
    const { user, dialog } = await startRegistration(api);
    const replace = await within(dialog).findByRole("button", { name: "replace registration" });
    expect(replace).toBeDisabled();
    expect(within(dialog).getByText("/relocated/source")).toBeVisible();
    expect(api.registerManifest).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("checkbox", { name: "Replace the existing registered snapshot." }));
    await user.click(replace);
    expect(api.registerManifest).toHaveBeenCalledWith(
      manifestFixture.manifestPath, manifestFixture.manifestHash, true,
    );
  });

  it("cancels file selection without treating it as an error or a registration", async () => {
    const api = makeApi();
    api.chooseManifest.mockResolvedValueOnce(null);
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("heading", { name: "No manifests yet." });
    await user.click(screen.getByRole("button", { name: "manifest" }));
    const dialog = screen.getByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "browse" }));
    expect(api.chooseManifest).toHaveBeenCalledOnce();
    expect(api.previewManifest).not.toHaveBeenCalled();
    expect(api.registerManifest).not.toHaveBeenCalled();
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
    fireEvent(dialog, new Event("cancel", { cancelable: true }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "manifest" })).toHaveFocus();
  });

  it("supports the native picker and automatically previews the selected file", async () => {
    const api = makeApi();
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("heading", { name: "No manifests yet." });
    await user.click(screen.getByRole("button", { name: "manifest" }));
    const dialog = screen.getByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "browse" }));
    expect(await within(dialog).findByText("valid manifest")).toBeVisible();
    expect(api.previewManifest).toHaveBeenCalledWith(manifestFixture.manifestPath);
    expect(api.registerManifest).not.toHaveBeenCalled();
  });

  it("unregisters only after confirmation and returns to the empty registry", async () => {
    const api = makeApi([registeredFixture]);
    const user = userEvent.setup();
    render(<App api={api} />);
    await user.click(await screen.findByRole("button", { name: "Atlas" }));
    await user.click(screen.getByRole("button", { name: "unregister" }));
    const dialog = screen.getByRole("dialog", { name: "unregister / atlas" });
    expect(api.unregisterApp).not.toHaveBeenCalled();
    expect(within(dialog).getByText(/Source files and the manifest stay untouched/)).toBeVisible();
    await user.click(within(dialog).getByRole("button", { name: "unregister" }));
    expect(await screen.findByRole("heading", { name: "No manifests yet." })).toBeVisible();
    expect(api.unregisterApp).toHaveBeenCalledWith("atlas");
  });

  it("shows a storage failure and supports retry", async () => {
    const api = makeApi([registeredFixture]);
    api.listApps.mockRejectedValueOnce({ code: "storage", message: "Registry is not writable." });
    const user = userEvent.setup();
    render(<App api={api} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Registry is not writable");
    expect(screen.queryByText("No manifests yet.")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "refresh" }));
    expect(await screen.findByRole("button", { name: "Atlas" })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps the last snapshot visible when a refresh fails", async () => {
    const api = makeApi([registeredFixture]);
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("button", { name: "Atlas" });
    api.listApps.mockRejectedValueOnce({ code: "storage", message: "Database is locked." });
    await user.click(screen.getByRole("button", { name: "refresh" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Database is locked");
    expect(screen.getByRole("button", { name: "Atlas" })).toBeVisible();
    expect(screen.getByText(/Showing the last loaded snapshot/)).toBeVisible();
  });

  it("does not pretend a normal browser is connected to native state", async () => {
    const api = makeApi([], false);
    render(<App api={api} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Use the Pipes desktop app");
    expect(screen.getByRole("button", { name: "manifest" })).toBeDisabled();
    expect(api.listApps).not.toHaveBeenCalled();
    expect(screen.queryByText("0 apps / 0 runs")).not.toBeInTheDocument();
  });

  it("supports stage inspection and keyboard navigation to the stored manifest", async () => {
    const user = userEvent.setup();
    render(<App api={makeApi([registeredFixture])} />);
    await user.click(await screen.findByRole("button", { name: "gamma: not run" }));
    expect(screen.getByRole("region", { name: "gamma configuration" })).toHaveTextContent("manual gate");
    const plan = screen.getByRole("tab", { name: "plan" });
    plan.focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "manifest" })).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByRole("tabpanel", { name: "manifest" });
    expect(panel).toBeVisible();
    expect(panel.querySelector("pre")?.textContent).toBe(manifestFixture.manifestYaml);
  });

  it("keeps fork connections through preview, registration, overview and join inspection", async () => {
    const api = makeApi([], true, forkManifestFixture);
    const { user, dialog } = await startRegistration(api);
    await within(dialog).findByText("valid manifest");
    expect(dialog.querySelectorAll(".stage-arrow")).toHaveLength(5);
    expect(dialog.querySelector('[data-from="linux"][data-to="macos"]')).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "register" }));
    await screen.findByText("Atlas registered. No commands ran.");
    await user.click(screen.getByRole("button", { name: "Pipelines" }));
    const flow = await screen.findByRole("list", { name: "Atlas stages" });
    expect(flow.parentElement?.querySelectorAll(".stage-arrow")).toHaveLength(5);
    await user.click(within(flow).getByRole("button", { name: "prod: not run" }));
    const details = screen.getByRole("region", { name: "prod configuration" });
    expect(details).toHaveTextContent("after all");
    expect(details).toHaveTextContent("linux");
    expect(details).toHaveTextContent("macos");
  });

  it("preserves composed check plans and limits through preview and registration", async () => {
    const api = makeApi([], true, composedManifestFixture);
    const { user, dialog } = await startRegistration(api);
    await within(dialog).findByText("valid manifest");
    expect(within(dialog).getByRole("group", { name: "quick-tests parallel group" })).toBeVisible();
    expect(within(dialog).getByRole("group", { name: "acceptance parallel group" })).toHaveTextContent("max 2 commands");
    expect(dialog.querySelectorAll(".check-barrier")).toHaveLength(2);
    expect(api.registerManifest).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "register" }));
    await screen.findByText("Atlas registered. No commands ran.");
    expect(screen.getByRole("group", { name: "quick-tests parallel group" })).toBeVisible();
    expect(screen.getByRole("list", { name: "package-checks ordered checks" })).toBeVisible();
    expect(screen.getByText("shared budget: max 8 commands")).toBeVisible();
  });

  it("wires the manifest library, real run history, and actual registry storage", async () => {
    const api = makeApi([registeredFixture]);
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("button", { name: "Atlas" });
    expect(screen.getByRole("button", { name: "run pipeline" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Manifest library" }));
    expect(screen.getByRole("region", { name: "Atlas manifest" })).toHaveTextContent(manifestFixture.manifestPath);
    await user.click(screen.getByRole("button", { name: "view atlas snapshot" }));
    expect(screen.getByRole("tab", { name: "manifest" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel", { name: "manifest" })).toHaveTextContent("api: pipes/v1");
    await user.click(screen.getByRole("button", { name: "Releases" }));
    expect(screen.getByRole("region", { name: "Recorded consumer deployments" })).toHaveTextContent("No deployments recorded.");
    expect(screen.getByRole("region", { name: "Pipeline run history" })).toHaveTextContent("No pipeline runs recorded.");
    expect(screen.getByText(/command success does not supply harness counts or bypass promotion policies/)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Local store" }));
    expect(screen.getByText("/state/pipes/registry.sqlite3")).toBeVisible();
    expect(screen.getByText(/Artifact storage is not enabled/)).toBeVisible();
    expect(api.registerManifest).not.toHaveBeenCalled();
    expect(api.unregisterApp).not.toHaveBeenCalled();
    expect(api.startPipelineRun).not.toHaveBeenCalled();
  });

  it("opens an explicit design preview without replacing an unavailable native registry", async () => {
    const api = makeApi([], false);
    const user = userEvent.setup();
    render(<App api={api} />);
    await screen.findByRole("alert");
    await user.click(screen.getByRole("button", { name: "design preview" }));
    await user.click(screen.getByRole("button", { name: "Manifest library" }));
    await user.click(screen.getByRole("button", { name: "Local store" }));
    expect(api.listApps).not.toHaveBeenCalled();
    expect(api.chooseManifest).not.toHaveBeenCalled();
    expect(api.previewManifest).not.toHaveBeenCalled();
    expect(api.registerManifest).not.toHaveBeenCalled();
    expect(api.unregisterApp).not.toHaveBeenCalled();
    expect(api.startPipelineRun).not.toHaveBeenCalled();
    expect(api.listPipelineRuns).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "exit preview" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Use the Pipes desktop app");
    expect(screen.getByRole("button", { name: "manifest" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /^atlas$/i })).not.toBeInTheDocument();
  });

  it("returns from design preview to the same real registration without simulated results", async () => {
    const api = makeApi([registeredFixture]);
    const user = userEvent.setup();
    render(<App api={api} />);
    await user.click(await screen.findByRole("button", { name: "gamma: not run" }));
    await user.click(screen.getByRole("button", { name: "prod: not run" }));
    await user.click(screen.getByRole("tab", { name: "manifest" }));
    await user.click(screen.getByRole("button", { name: "design preview" }));
    await user.click(screen.getByRole("button", { name: "Releases" }));
    await user.click(screen.getByRole("button", { name: "exit preview" }));
    expect(screen.getByRole("region", { name: "prod configuration" })).toBeVisible();
    expect(screen.getByRole("button", { name: "prod: not run" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("tab", { name: "manifest" })).toHaveAttribute("aria-selected", "true");
    expect(api.registerManifest).not.toHaveBeenCalled();
    expect(api.unregisterApp).not.toHaveBeenCalled();
    expect(screen.queryByText("RUNNING", { exact: true })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "design preview" })).toHaveFocus());
  });
});
