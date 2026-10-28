import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { consumerDeploymentFixture, consumerFixture, consumerManifestFixture } from "../test/fixtures";
import { ConsumerPanel, ConsumerRoutes, DeploymentHistory } from "./ConsumerDeployments";

const beta = consumerManifestFixture.stages.find((stage) => stage.id === "beta");
if (!beta) throw new Error("Beta fixture is missing.");

describe("consumer destinations and history", () => {
  it("connects a named destination to its source stage for inspection", async () => {
    const select = vi.fn();
    const user = userEvent.setup();
    render(<ConsumerRoutes stages={consumerManifestFixture.stages} selectedStage="test" onSelectStage={select} />);
    await user.click(screen.getByRole("button", { name: "Inspect Personal MacBook consumer of beta" }));
    expect(select).toHaveBeenCalledWith("beta");
  });

  it("requires registration before recording and explains that consumers do not gate the pipeline", () => {
    render(<ConsumerPanel appId="atlas" stage={beta} records={[]} />);
    expect(screen.getByRole("button", { name: "deployed to Personal MacBook" })).toBeDisabled();
    expect(screen.getByText(/Register this manifest to record/)).toBeVisible();
    expect(screen.getByText(/Recording delivery does not advance stages. Promotion criteria may require its marker/)).toBeVisible();
  });

  it("shows the latest marker only for the current destination", async () => {
    const onRecord = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(<ConsumerPanel appId="atlas" stage={beta} records={[consumerDeploymentFixture]} onRecord={onRecord} />);
    expect(screen.getByText("last manual record")).toBeVisible();
    expect(screen.getByText("v1.5.0")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "deployed to Personal MacBook" }));
    expect(onRecord).toHaveBeenCalledWith(consumerFixture);
    rerender(<ConsumerPanel
      appId="atlas"
      stage={{ ...beta, consumers: [{ ...consumerFixture, target: "Replacement laptop" }] }}
      records={[consumerDeploymentFixture]}
      onRecord={onRecord}
    />);
    expect(screen.getByText("no deployment recorded to this destination")).toBeVisible();
    expect(screen.queryByText("v1.5.0")).not.toBeInTheDocument();
  });

  it("keeps historical target and provenance visible after the app is unregistered", async () => {
    const user = userEvent.setup();
    render(<DeploymentHistory records={[consumerDeploymentFixture]} registeredAppIds={new Set()} />);
    const history = screen.getByRole("list", { name: "Manual consumer deployment history" });
    expect(within(history).getByText("app unregistered")).toBeVisible();
    expect(within(history).getByText(consumerFixture.target, { selector: "code" })).toBeVisible();
    await user.click(within(history).getByText("record #1 / provenance + note"));
    expect(within(history).getByText(consumerDeploymentFixture.manifestHash)).toBeVisible();
    expect(within(history).getByText(consumerDeploymentFixture.manifestPath)).toBeVisible();
    expect(within(history).getByText("Copied the beta build to my laptop.")).toBeVisible();
    expect(within(history).getByText(/Delivery, artifacts, and stage outcomes were not verified/)).toBeVisible();
  });
});
