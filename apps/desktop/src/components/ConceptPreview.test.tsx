import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConceptPreview } from "./ConceptPreview";
import { desktopApi } from "../lib/api";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function expectPreviewLabels() {
  expect(screen.getByText("DESIGN PREVIEW")).toBeVisible();
  expect(screen.getByRole("region", { name: "Design preview notice" })).toHaveTextContent(
    "Simulation only. No commands, downloads, or registry changes. Nothing is persisted.",
  );
  expect(screen.getByText("simulation only", { exact: true })).toBeVisible();
  expect(screen.queryByText(/daemon online/i)).not.toBeInTheDocument();
}

async function openAtlas(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Open atlas sample release" }));
}

async function approveSample(user: ReturnType<typeof userEvent.setup>) {
  const dialog = screen.getByRole("dialog", { name: "approve promotion / gamma" });
  const submit = within(dialog).getByRole("button", { name: "simulate promotion approval" });
  expect(submit).toBeDisabled();
  await user.click(within(dialog).getByRole("checkbox", { name: /I approve this promotion-criteria decision/ }));
  expect(submit).toBeDisabled();
  await user.click(within(dialog).getByRole("checkbox", { name: /I acknowledge that these optional steps are skipped/ }));
  await user.click(submit);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
}

async function reportCanary(
  user: ReturnType<typeof userEvent.setup>,
  passed: number,
  total: number,
  releaseRef?: string,
) {
  await user.click(screen.getByRole("button", { name: "Report results for canary" }));
  const dialog = screen.getByRole("dialog", { name: "report harness / canary" });
  const reported = within(dialog).getByRole("spinbutton", { name: "Tests reported" });
  const passing = within(dialog).getByRole("spinbutton", { name: "Tests passed" });
  expect(reported).toHaveDisplayValue("");
  expect(passing).toHaveDisplayValue("");
  if (releaseRef) {
    await user.clear(within(dialog).getByRole("textbox", { name: "Release reference" }));
    await user.type(within(dialog).getByRole("textbox", { name: "Release reference" }), releaseRef);
  }
  await user.type(reported, String(total));
  await user.type(passing, String(passed));
  await user.click(within(dialog).getByRole("button", { name: "save sample results" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
}

async function requestPromotion(user: ReturnType<typeof userEvent.setup>, surface: string) {
  if (surface === "overview conduit") await user.click(screen.getByRole("button", { name: "01 / overview" }));
  const name = surface === "toolbar" ? "Simulate approve & promote"
    : surface === "inspector" ? "simulate: approve & promote" : "gamma to prod: approve (preview)";
  await user.click(screen.getByRole("button", { name }));
}

describe("isolated interactive design preview", () => {
  it("shows the three sample pipelines with truthful initial counts and preview labeling", () => {
    render(<ConceptPreview onExit={vi.fn()} />);
    expectPreviewLabels();
    expect(screen.getByRole("heading", { level: 1, name: "pipelines_" })).toHaveFocus();
    for (const id of ["atlas", "beacon", "relay"]) {
      expect(screen.getByRole("region", { name: `${id} sample pipeline` })).toBeVisible();
    }
    expect(screen.getByText("3 sample projects")).toBeVisible();
    expect(screen.getByText("1 executing & promoting")).toBeVisible();
    expect(screen.getByText("1 failed pipeline")).toBeVisible();
    expect(screen.getByText("1 live candidate")).toBeVisible();
    expect(screen.getByRole("heading", { name: "SAMPLE TELEMETRY" })).toBeVisible();
  });

  it.each([
    { id: "atlas", version: "1.5.0", stage: "gamma", commit: "8f1c2a9", checks: 4, artifacts: 2 },
    { id: "beacon", version: "2.8.1", stage: "test", commit: "b70d3e1", checks: 2, artifacts: 1 },
    { id: "relay", version: "0.9.3", stage: "prod", commit: "c42e90a", checks: 4, artifacts: 2 },
  ])("opens $id's own release, metadata, evidence, and receipt", async ({ id, version, stage, commit, checks, artifacts }) => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: `Open ${id} sample release` }));
    expect(screen.getByRole("heading", { level: 1, name: `${id} / v${version}` })).toHaveFocus();
    expect(screen.getByText(`commit ${commit}`)).toBeVisible();
    expect(screen.getByRole("tabpanel", { name: `checks (${checks})` })).toBeVisible();
    expect(screen.getByRole("tab", { name: `artifacts (${artifacts})` })).toBeVisible();
    expect(within(screen.getByRole("complementary", { name: "Selected sample stage" })).getByRole("heading", { name: stage })).toBeVisible();
    expect(screen.getByText("sample cryptographic receipt")).toBeVisible();
    const receipt = screen.getByText("sample cryptographic receipt").closest("details")!;
    expect(receipt).toHaveTextContent(commit);
    const terminal = screen.getByLabelText(`${id} sample execution stream`);
    expect(terminal).toHaveTextContent(`${id} v${version}`);
    expect(terminal).toHaveTextContent("Sample output only");
    if (id !== "atlas") expect(screen.queryByRole("group", { name: "Sample stage state preset" })).not.toBeInTheDocument();
    expectPreviewLabels();
  });

  it("opens the clicked project's stage rather than always opening Atlas", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    const beacon = screen.getByRole("region", { name: "beacon sample pipeline" });
    await user.click(within(beacon).getByRole("button", { name: /^prod:/i }));
    expect(screen.getByRole("heading", { level: 1, name: "beacon / v2.8.1" })).toHaveFocus();
    const inspector = screen.getByRole("complementary", { name: "Selected sample stage" });
    expect(within(inspector).getByRole("heading", { name: "prod" })).toBeVisible();
    expect(inspector).toHaveTextContent("v2.7.4 (sample)");
    expect(within(inspector).getByRole("button", { name: "prod untouched" })).toBeDisabled();
    expect(screen.getByRole("tabpanel", { name: "checks (2)" })).toHaveTextContent("2 assertions failed");
    await user.click(screen.getByRole("button", { name: "01 / overview" }));
    const relay = screen.getByRole("region", { name: "relay sample pipeline" });
    await user.click(within(relay).getByRole("button", { name: /^beta:/i }));
    expect(screen.getByRole("heading", { level: 1, name: "relay / v0.9.3" })).toHaveFocus();
    expect(within(screen.getByRole("complementary", { name: "Selected sample stage" })).getByRole("heading", { name: "beta" })).toBeVisible();
  });

  it("synchronizes stage cards, checks, inspector, terminal, history, and overview when presets change", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    expect(screen.getByRole("tabpanel", { name: "checks (4)" })).toHaveTextContent("running 74%");
    const inspector = screen.getByRole("complementary", { name: "Selected sample stage" });
    expect(inspector).toHaveTextContent("74%");
    await user.click(screen.getByRole("button", { name: "02 approval gate" }));
    expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /^gamma:/i })).toHaveAccessibleName(/GATE/i);
    const checks = screen.getByRole("tabpanel", { name: "checks (4)" });
    expect(checks).toHaveTextContent("4/4 checks complete");
    expect(checks).not.toHaveTextContent("running 74%");
    expect(inspector).toHaveTextContent("awaiting approval");
    expect(screen.getByLabelText("atlas sample execution stream")).toHaveTextContent("canary 4/4 passed · awaiting approval");
    await user.click(screen.getByRole("tab", { name: "history" }));
    expect(screen.getByRole("tabpanel", { name: "history" })).toHaveTextContent("Sample history for atlas / v1.5.0");
    expect(screen.getByRole("tabpanel", { name: "history" })).toHaveTextContent("awaiting approval");
    await user.click(screen.getByRole("button", { name: "03 prod promoting" }));
    expect(screen.getByRole("button", { name: /^prod:/i })).toHaveAccessibleName(/DEPLOYING/i);
    expect(within(inspector).getByRole("heading", { name: "prod" })).toBeVisible();
    expect(inspector).toHaveTextContent("62%");
    expect(inspector).toHaveTextContent("v1.4.2 (sample)");
    expect(screen.getByLabelText("atlas sample execution stream")).toHaveTextContent("executing at 62%");
    expect(screen.getByRole("tabpanel", { name: "history" })).toHaveTextContent("visual scenario snapshot");
    expect(screen.queryByRole("list", { name: "Sample promotion approval history" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "checks (5)" }));
    expect(screen.getByRole("tabpanel", { name: "checks (5)" })).toHaveTextContent("running 62%");
    await user.click(screen.getByRole("button", { name: "01 / overview" }));
    expect(screen.getByRole("region", { name: "atlas sample pipeline" })).toHaveTextContent("promoting → prod");
    expect(screen.getByText("1 executing & promoting")).toBeVisible();
    expectPreviewLabels();
  });

  it("advances only in explicit simulated steps, finishes live, and resets with Run pipeline", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "Simulate complete checks" }));
    expect(screen.getByRole("status")).toHaveTextContent("The approval gate is waiting");
    await user.click(screen.getByRole("button", { name: "Simulate approve & promote" }));
    await approveSample(user);
    expect(screen.getByRole("status")).toHaveTextContent("Prod promotion is at 62%");
    await user.click(screen.getByRole("button", { name: "Simulate finish deployment" }));
    expect(screen.getByRole("button", { name: /^prod:/i })).toHaveAccessibleName(/LIVE/i);
    expect(screen.getByRole("status")).toHaveTextContent("No deployment occurred");
    expect(screen.getByRole("tabpanel", { name: "checks (5)" })).toHaveTextContent("release healthy · v1.5.0 active");
    expect(screen.getByRole("complementary", { name: "Selected sample stage" })).toHaveTextContent("v1.5.0 (sample)");
    await user.click(screen.getByRole("button", { name: "01 / overview" }));
    expect(screen.getByText("0 executing & promoting")).toBeVisible();
    expect(screen.getByText("2 live candidates")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "run pipeline" }));
    expect(screen.getByRole("status")).toHaveTextContent("No pipeline or command started");
    expect(screen.getByRole("region", { name: "atlas sample pipeline" })).toHaveTextContent("canary 3/4 checks");
    expect(screen.getByText("1 executing & promoting")).toBeVisible();
    expect(screen.getByText("1 live candidate")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Dismiss preview notice" }));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "pipelines_" })).toHaveFocus();
    expectPreviewLabels();
  });

  it("makes actionable conduits advance the sample and navigate to the correct release", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    const atlas = screen.getByRole("region", { name: "atlas sample pipeline" });
    await user.click(within(atlas).getByRole("button", { name: /beta.*gamma/i }));
    expect(screen.getByRole("heading", { level: 1, name: "atlas / v1.5.0" })).toHaveFocus();
    expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: /gamma.*prod/i }));
    await approveSample(user);
    expect(screen.getByRole("button", { name: "03 prod promoting" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: /gamma.*prod/i }));
    expect(screen.getByRole("button", { name: /^prod:/i })).toHaveAccessibleName(/LIVE/i);
    expect(screen.queryByRole("button", { name: /gamma.*prod/i })).not.toBeInTheDocument();
  });

  it("supports arrow, Home, End, and wrapping keyboard tab navigation with matching panels", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    screen.getByRole("tab", { name: "checks (4)" }).focus();
    await user.keyboard("{ArrowRight}");
    const artifacts = screen.getByRole("tab", { name: "artifacts (2)" });
    expect(artifacts).toHaveFocus();
    expect(artifacts).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel", { name: "artifacts (2)" })).toHaveAttribute("id", artifacts.getAttribute("aria-controls"));
    expect(screen.getByRole("tabpanel", { name: "artifacts (2)" })).toHaveAttribute("aria-labelledby", artifacts.id);
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "manifest" })).toHaveFocus();
    expect(screen.getByRole("tabpanel", { name: "manifest" })).toHaveTextContent("app: atlas");
    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "history" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "checks (4)" })).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "history" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: "checks (4)" })).toHaveFocus();
    expect(screen.getAllByRole("tab").filter((element) => element.tabIndex === 0)).toHaveLength(1);
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
  });

  it("selects sample artifacts without fetching, downloading, opening a file picker, or persisting", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const anchorClick = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const storageWrite = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {});
    const showDialog = vi.spyOn(HTMLDialogElement.prototype, "showModal");
    const user = userEvent.setup();
    const { container } = render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "fetch artifact" }));
    expect(screen.getByRole("tab", { name: "artifacts (2)" })).toHaveFocus();
    expect(screen.getByRole("button", { name: "Select sample Atlas-1.5.0-macos-arm64.zip" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("status")).toHaveTextContent("No file was fetched or downloaded");
    await user.click(screen.getByRole("button", { name: "Select sample test-results.xcresult.zip" }));
    expect(screen.getByRole("status")).toHaveTextContent("test-results.xcresult.zip selected");
    expect(screen.getByRole("button", { name: "Select sample test-results.xcresult.zip" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: "01 / overview" }));
    await user.click(screen.getByRole("button", { name: "manifest" }));
    expect(screen.getByRole("heading", { level: 1, name: "sample manifest / atlas" })).toHaveFocus();
    expect(screen.getByText(/not a file picker/)).toBeVisible();
    expect(fetch).not.toHaveBeenCalled();
    expect(anchorClick).not.toHaveBeenCalled();
    expect(storageWrite).not.toHaveBeenCalled();
    expect(showDialog).not.toHaveBeenCalled();
    expect(container.querySelector("[download]")).toBeNull();
  });

  it("keeps sidebar manifest and storage views clearly labeled and preserves project selection", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Manifest library" }));
    expectPreviewLabels();
    expect(screen.getByRole("heading", { level: 1, name: "sample manifest / atlas" })).toHaveFocus();
    await user.click(within(screen.getByRole("group", { name: "Choose sample manifest" })).getByRole("button", { name: "beacon" }));
    expect(screen.getByRole("heading", { level: 1, name: "sample manifest / beacon" })).toHaveFocus();
    expect(screen.getByLabelText("beacon sample manifest")).toHaveTextContent("app: beacon");
    expect(screen.getByLabelText("beacon sample manifest")).not.toHaveTextContent("app: atlas");
    await user.click(screen.getByRole("button", { name: "Local store" }));
    expectPreviewLabels();
    expect(screen.getByRole("heading", { level: 1, name: "sample storage" })).toHaveFocus();
    expect(screen.getByText("0 bytes written")).toBeVisible();
    expect(screen.getByRole("region", { name: "Sample storage summary" })).toHaveTextContent("41.8 MB represented");
    await user.click(screen.getByRole("button", { name: "Inspect relay sample release" }));
    expect(screen.getByRole("heading", { level: 1, name: "relay / v0.9.3" })).toHaveFocus();
    expectPreviewLabels();
  });

  it("exits from any view and starts a fresh simulation after remount", async () => {
    const user = userEvent.setup();
    const onExit = vi.fn();
    const first = render(<ConceptPreview onExit={onExit} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "03 prod promoting" }));
    await user.click(screen.getByRole("button", { name: "Simulate finish deployment" }));
    await user.click(screen.getByRole("button", { name: "Local store" }));
    await user.click(screen.getByRole("button", { name: "exit preview" }));
    expect(onExit).toHaveBeenCalledOnce();
    first.unmount();
    render(<ConceptPreview onExit={onExit} />);
    expectPreviewLabels();
    expect(screen.getByText("1 executing & promoting")).toBeVisible();
    await openAtlas(user);
    expect(screen.getByRole("button", { name: "01 executing + promoting" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("tabpanel", { name: "checks (4)" })).toHaveTextContent("running 74%");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("simulates manually approved consumer records without native writes or stage promotion", async () => {
    const nativeWrite = vi.spyOn(desktopApi, "recordConsumerDeployment");
    const storageWrite = vi.spyOn(Storage.prototype, "setItem");
    const user = userEvent.setup();
    const first = render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "Inspect Personal MacBook consumer of beta" }));
    expect(screen.getByRole("region", { name: "beta consumers" })).toHaveTextContent("v1.4.2");
    await user.click(screen.getByRole("button", { name: "deployed to Personal MacBook" }));
    const dialog = screen.getByRole("dialog", { name: "deployed to / Personal MacBook" });
    expect(dialog).toHaveTextContent("Simulation only: this marker is not saved.");
    expect(within(dialog).getByRole("textbox", { name: "Release reference" })).toHaveValue("v1.5.0");
    expect(within(dialog).getByRole("button", { name: "simulate approval & record" })).toBeDisabled();
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "simulate approval & record" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "history" })).toHaveAttribute("aria-selected", "true");
    const history = screen.getByRole("list", { name: "Sample consumer deployment history" });
    const records = within(history).getAllByRole("listitem");
    expect(records).toHaveLength(3);
    const betaRecords = records.filter((record) => within(record).queryByText("atlas / beta"));
    expect(betaRecords).toHaveLength(2);
    expect(betaRecords[0]).toHaveTextContent("v1.5.0");
    expect(betaRecords[1]).toHaveTextContent("v1.4.2");
    expect(records.filter((record) => within(record).queryByText("atlas / gamma"))).toHaveLength(1);
    expect(screen.getByRole("region", { name: "beta consumers" })).toHaveTextContent("v1.5.0");
    expect(screen.getByRole("button", { name: "beta: PASSED" })).toBeVisible();
    expect(screen.getByRole("button", { name: "gamma: RUNNING" })).toBeVisible();
    expect(nativeWrite).not.toHaveBeenCalled();
    expect(storageWrite).not.toHaveBeenCalled();
    first.unmount();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "Inspect Personal MacBook consumer of beta" }));
    expect(screen.getByRole("region", { name: "beta consumers" })).toHaveTextContent("v1.4.2");
    expect(screen.getByRole("region", { name: "beta consumers" })).not.toHaveTextContent("v1.5.0");
  });
});

describe("preview promotion criteria and acknowledgements", () => {
  it("shows candidate-specific seeded readiness with the exact harness threshold and optional delivery warning", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    const panel = screen.getByRole("region", { name: "gamma promotion criteria" });
    expect(panel).toHaveTextContent("at least 45 pass / exactly 50 tests");
    expect(panel).toHaveTextContent("required deployment");
    expect(panel).toHaveTextContent("optional deployment");
    expect(within(panel).getByRole("textbox", { name: "Candidate release reference" })).toHaveValue("v1.5.0");
    await user.click(within(panel).getByRole("button", { name: "assess readiness" }));
    const assessed = screen.getByRole("region", { name: "Promotion readiness assessment" });
    expect(assessed).toHaveTextContent("eligible with warnings");
    expect(assessed).toHaveTextContent("45/50 passed; minimum 45 met");
    expect(assessed).toHaveTextContent("Manual deployment to Lab Workstation is recorded for v1.5.0");
    expect(assessed).toHaveTextContent("Optional deployment to Personal Canary Device has no current marker");
    expectPreviewLabels();
  });

  it.each(["toolbar", "inspector", "detail conduit", "overview conduit"])(
    "requires both acknowledgements before advancing from the %s",
    async (surface) => {
      const user = userEvent.setup();
      render(<ConceptPreview onExit={vi.fn()} />);
      await openAtlas(user);
      await user.click(screen.getByRole("button", { name: "02 approval gate" }));
      await requestPromotion(user, surface);
      const dialog = screen.getByRole("dialog", { name: "approve promotion / gamma" });
      expect(dialog).toHaveTextContent("atlas / v1.5.0");
      expect(dialog).toHaveTextContent("personal-delivery");
      expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
      await approveSample(user);
      expect(screen.getByRole("button", { name: "03 prod promoting" })).toHaveAttribute("aria-pressed", "true");
      await user.click(screen.getByRole("tab", { name: "history" }));
      const history = screen.getByRole("list", { name: "Sample promotion approval history" });
      expect(history.querySelectorAll(":scope > li")).toHaveLength(1);
      expect(history).toHaveTextContent("atlas / gamma");
      expect(history).toHaveTextContent("Acknowledged skipped optional steps: personal-delivery");
    },
  );

  it.each(["toolbar", "inspector", "detail conduit", "overview conduit"])(
    "keeps a failed required harness at the gate when using the %s",
    async (surface) => {
      const user = userEvent.setup();
      render(<ConceptPreview onExit={vi.fn()} />);
      await openAtlas(user);
      await user.click(screen.getByRole("button", { name: "02 approval gate" }));
      await user.click(screen.getByRole("tab", { name: "promotion" }));
      await reportCanary(user, 44, 50);
      await requestPromotion(user, surface);
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
      expect(screen.getByRole("status")).toHaveTextContent("44/50 passed; at least 45 must pass");
      expect(screen.getByRole("status")).toHaveTextContent("No sample stage advanced");
      await user.click(screen.getByRole("tab", { name: "history" }));
      expect(screen.queryByRole("list", { name: "Sample promotion approval history" })).not.toBeInTheDocument();
      const reports = screen.getByRole("list", { name: "Sample reported harness results" });
      expect(within(reports).getAllByRole("listitem")).toHaveLength(2);
      expect(within(reports).getAllByRole("listitem")[0]).toHaveTextContent("44/50 reported passes");
    },
  );

  it("uses Atlas gamma and the real sample candidate when the toolbar is clicked while another stage is selected", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "02 approval gate" }));
    await user.click(screen.getByRole("button", { name: "beta: PASSED" }));
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await user.clear(screen.getByRole("textbox", { name: "Candidate release reference" }));
    await user.type(screen.getByRole("textbox", { name: "Candidate release reference" }), "other-candidate");
    await requestPromotion(user, "toolbar");
    const dialog = screen.getByRole("dialog", { name: "approve promotion / gamma" });
    expect(dialog).toHaveTextContent("atlas / v1.5.0");
    expect(dialog).not.toHaveTextContent("other-candidate");
    expect(dialog).toHaveTextContent("45/50 passed");
    await user.click(within(dialog).getByRole("button", { name: "cancel" }));
    expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("tab", { name: "history" }));
    expect(screen.queryByRole("list", { name: "Sample promotion approval history" })).not.toBeInTheDocument();
  });

  it("never reuses seeded evidence for another candidate and records exact external counts without filling them from policy", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    const reference = screen.getByRole("textbox", { name: "Candidate release reference" });
    await user.clear(reference);
    await user.type(reference, "v2.0.0");
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(screen.getByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("No current reported results for canary and v2.0.0");
    await reportCanary(user, 45, 49);
    expect(screen.getByRole("status")).toHaveTextContent("atlas / gamma / v2.0.0");
    await waitFor(() => expect(screen.queryByRole("region", { name: "Promotion readiness assessment" })).not.toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    const assessed = screen.getByRole("region", { name: "Promotion readiness assessment" });
    expect(assessed).toHaveTextContent("Expected exactly 50 tests; 49 were reported");
    expect(assessed).toHaveTextContent("Required deployment to Lab Workstation has no current marker for v2.0.0");
    expect(screen.getByRole("button", { name: "review promotion approval" })).toBeDisabled();
    await user.click(screen.getByRole("tab", { name: "history" }));
    const report = within(screen.getByRole("list", { name: "Sample reported harness results" })).getAllByRole("listitem")[0]!;
    expect(report).toHaveTextContent("45/49 reported passes");
    expect(report).toHaveTextContent("v2.0.0");
    expect(report).toHaveTextContent("/preview/atlas/pipes.yaml");
    expect(report).toHaveTextContent("/preview/atlas");
    expect(report).toHaveTextContent("2026-10-26T10:00:00Z");
  });

  it("records panel decisions without advancing and preserves frozen skips after optional delivery", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "02 approval gate" }));
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    await user.click(screen.getByRole("button", { name: "review optional skips & approve" }));
    await approveSample(user);
    expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("status")).toHaveTextContent("The sample stage is unchanged");
    expect(screen.getByRole("list", { name: "Sample promotion approval history" })).toHaveTextContent("personal-delivery");

    await user.click(screen.getByRole("button", { name: "deployed to Personal Canary Device" }));
    const delivery = screen.getByRole("dialog", { name: "deployed to / Personal Canary Device" });
    await user.click(within(delivery).getByRole("checkbox"));
    await user.click(within(delivery).getByRole("button", { name: "simulate approval & record" }));
    const approvals = screen.getByRole("list", { name: "Sample promotion approval history" });
    expect(approvals).toHaveTextContent("approved with optional skips");
    expect(approvals).toHaveTextContent("Acknowledged skipped optional steps: personal-delivery");
    await user.click(within(approvals).getByText(/frozen decision/));
    const frozen = within(approvals).getByRole("region", { name: "Recorded promotion assessment" });
    expect(frozen).toHaveTextContent("were acknowledged as skipped when this approval was recorded");
    expect(frozen).toHaveTextContent("Personal Canary Device has no current marker");
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    const current = screen.getByRole("region", { name: "Promotion readiness assessment" });
    expect(current).toHaveTextContent("criteria satisfied");
    expect(current).toHaveTextContent("Manual deployment to Personal Canary Device is recorded");
    expect(screen.getByRole("button", { name: "02 approval gate" })).toHaveAttribute("aria-pressed", "true");
  });

  it("does not fabricate an approval when selecting visual presets", async () => {
    const user = userEvent.setup();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "03 prod promoting" }));
    expect(screen.getByRole("status")).toHaveTextContent("No approval was recorded");
    expect(screen.getByText("Visual scenario presets only — selecting a preset does not record an approval.")).toBeVisible();
    await user.click(screen.getByRole("tab", { name: "history" }));
    expect(screen.getByText("No promotion-criteria approvals recorded.")).toBeVisible();
    expect(screen.queryByRole("list", { name: "Sample promotion approval history" })).not.toBeInTheDocument();
    expect(screen.getByRole("tabpanel", { name: "history" })).not.toHaveTextContent("gamma approved in simulation");
  });

  it("keeps reports, assessments, approvals and deployments isolated and resets all records on remount", async () => {
    const nativeCalls = [
      vi.spyOn(desktopApi, "recordHarnessResult"),
      vi.spyOn(desktopApi, "assessPromotion"),
      vi.spyOn(desktopApi, "approvePromotion"),
      vi.spyOn(desktopApi, "recordConsumerDeployment"),
      vi.spyOn(desktopApi, "listHarnessResults"),
      vi.spyOn(desktopApi, "listPromotionApprovals"),
    ];
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const storageWrite = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {});
    const user = userEvent.setup();
    const first = render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("button", { name: "02 approval gate" }));
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await reportCanary(user, 45, 50);
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    await user.click(screen.getByRole("button", { name: "review optional skips & approve" }));
    await approveSample(user);
    await user.click(screen.getByRole("button", { name: "deployed to Personal Canary Device" }));
    const deployment = screen.getByRole("dialog", { name: "deployed to / Personal Canary Device" });
    await user.click(within(deployment).getByRole("checkbox"));
    await user.click(within(deployment).getByRole("button", { name: "simulate approval & record" }));
    await user.click(screen.getByRole("button", { name: "Local store" }));
    expect(screen.getByRole("region", { name: "Sample storage summary" })).toHaveTextContent("2 sample harness reports / 1 sample approvals");
    for (const call of nativeCalls) expect(call).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
    expect(storageWrite).not.toHaveBeenCalled();
    first.unmount();
    render(<ConceptPreview onExit={vi.fn()} />);
    await openAtlas(user);
    await user.click(screen.getByRole("tab", { name: "history" }));
    expect(within(screen.getByRole("list", { name: "Sample reported harness results" })).getAllByRole("listitem")).toHaveLength(1);
    expect(within(screen.getByRole("list", { name: "Sample consumer deployment history" })).getAllByRole("listitem")).toHaveLength(2);
    expect(screen.getByText("No promotion-criteria approvals recorded.")).toBeVisible();
    expect(screen.getByRole("button", { name: "01 executing + promoting" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("tab", { name: "promotion" }));
    await user.click(screen.getByRole("button", { name: "assess readiness" }));
    expect(screen.getByRole("region", { name: "Promotion readiness assessment" })).toHaveTextContent("eligible with warnings");
    expectPreviewLabels();
  });
});
