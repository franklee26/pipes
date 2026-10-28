import { describe, expect, it } from "vitest";
import { consumerDeploymentFixture, consumerFixture, consumerRegisteredFixture } from "../test/fixtures";
import { deploymentsForStage, latestConsumerDeployment, reviewConsumerDeployment } from "./consumerDeployments";

describe("consumer deployment context", () => {
  it("captures the reviewed destination and revision instead of following later edits", () => {
    const app = structuredClone(consumerRegisteredFixture);
    const consumer = structuredClone(consumerFixture);
    const review = reviewConsumerDeployment(app, "beta", consumer);
    consumer.name = "Renamed device";
    consumer.target = "Different device";
    app.manifest.manifestPath = "/moved/pipes.yaml";
    app.updatedAt = "2026-10-28T12:00:00Z";
    expect(review.consumer).toEqual(consumerFixture);
    expect(review.manifestPath).toBe(consumerRegisteredFixture.manifest.manifestPath);
    expect(review.updatedAt).toBe(consumerRegisteredFixture.updatedAt);
    expect(review.manifestHash).toBe(consumerRegisteredFixture.manifest.manifestHash);
  });

  it("does not attribute another app, stage, consumer, or destination's record to a target", () => {
    const original = structuredClone(consumerDeploymentFixture);
    const records = [
      { ...original, id: "5", appId: "different-app", releaseRef: "wrong-app" },
      { ...original, id: "4", stageId: "prod", releaseRef: "wrong-stage" },
      { ...original, id: "3", consumer: { ...original.consumer, id: "different-id" }, releaseRef: "wrong-id" },
      { ...original, id: "2", consumer: { ...original.consumer, target: "another device" }, releaseRef: "wrong-target" },
      original,
    ];
    expect(latestConsumerDeployment(records, "atlas", "beta", consumerFixture)).toBe(original);
    expect(latestConsumerDeployment(records, "atlas", "beta", { ...consumerFixture, target: "new device" })).toBeUndefined();
    expect(latestConsumerDeployment(records, "atlas", "beta", { ...consumerFixture, name: "New display name" })).toBe(original);
    expect(deploymentsForStage(records, "atlas", "beta").map((record) => record.id)).toEqual(["3", "2", "1"]);
  });
});
