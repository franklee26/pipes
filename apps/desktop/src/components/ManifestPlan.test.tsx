import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { forkManifestFixture, manifestFixture, unitCommandFixture } from "../test/fixtures";
import { ManifestPlan } from "./ManifestPlan";

describe("manifest inspection", () => {
  it("shows all-parent joins and approval intent without claiming execution evidence", async () => {
    const user = userEvent.setup();
    render(<ManifestPlan manifest={forkManifestFixture} initialStage="prod" showManifest />);
    const configuration = screen.getByRole("region", { name: "prod configuration" });
    expect(within(configuration).getByText("after all")).toBeVisible();
    expect(within(configuration).getByText("linux")).toBeVisible();
    expect(within(configuration).getByText("macos")).toBeVisible();
    expect(within(configuration).getByText("manual gate")).toBeVisible();
    expect(within(configuration).getByRole("button", { name: "run stage" })).toBeDisabled();
    expect(screen.getByText("not signed")).toBeVisible();
    expect(screen.getByText("not verified here")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "linux: not run" }));
    expect(screen.getByRole("region", { name: "linux configuration" })).toBeVisible();
    expect(screen.getByText("not required")).toBeVisible();
  });

  it("keeps planned outputs distinct from built artifacts", async () => {
    const user = userEvent.setup();
    render(<ManifestPlan manifest={{
      ...manifestFixture,
      outputs: [{ name: "desktop", path: "dist/Atlas.app", format: "directory" }],
    }} showManifest />);
    await user.click(screen.getByRole("tab", { name: "outputs (1)" }));
    const outputs = screen.getByRole("tabpanel", { name: "outputs (1)" });
    expect(within(outputs).getByText("dist/Atlas.app")).toBeVisible();
    expect(within(outputs).getByText("directory")).toBeVisible();
    expect(outputs).toHaveTextContent("This configuration view does not verify builds or store artifacts.");
    expect(screen.queryByRole("tabpanel", { name: "plan" })).not.toBeInTheDocument();
  });

  it("supports tab keyboard navigation and preserves the selected stage", async () => {
    const user = userEvent.setup();
    render(<ManifestPlan manifest={manifestFixture} initialStage="gamma" showManifest />);
    screen.getByRole("tab", { name: "plan" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "manifest" })).toHaveFocus();
    expect(screen.getByRole("tabpanel", { name: "manifest" }).querySelector("pre")?.textContent).toBe(manifestFixture.manifestYaml);
    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "outputs (0)" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: "plan" })).toHaveFocus();
    expect(screen.getByRole("button", { name: "gamma: not run" })).toHaveAttribute("aria-pressed", "true");
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "outputs (0)" })).toHaveFocus();
  });

  it("retains preparation commands in registration previews without duplicate manifest tabs", async () => {
    const user = userEvent.setup();
    render(<ManifestPlan manifest={{ ...manifestFixture, prepare: [unitCommandFixture] }} />);
    expect(screen.queryByRole("tab", { name: "manifest" })).not.toBeInTheDocument();
    const prepare = screen.getByText("prepare / 1 steps");
    await user.click(prepare);
    const disclosure = prepare.closest("details");
    if (!disclosure) throw new Error("Preparation disclosure is missing.");
    expect(disclosure).toHaveAttribute("open");
    expect(within(disclosure).getByText('["cargo","test"]')).toBeVisible();
  });
});
