# Pipes UI/UX Design Study (Concept v3)

This directory contains the interactive HTML concept and rendered screenshots for the upgraded **Pipes** retro-modern UI/UX:

- [pipes-concepts.html](./pipes-concepts.html) — Interactive prototype with `#overview` and `#release` screens, stage presets, executing stage loaders, and fluid promotion arrows.
- [pipes-overview.png](./pipes-overview.png) — Full-page render of `01 / overview` (`#overview`).
- [pipes-release.png](./pipes-release.png) — Full-page render of `02 / release` (`#release`).

## Summary of Design Upgrades

1. **Modernized Readability with Retro Monospace Typography**
   - Preserves `"Courier New", Courier, monospace` with `tabular-nums` while sharpening contrast (`--ink: #193327`, `--muted: #526356`), increasing label legibility, and introducing layered card surfaces (`--surface-raised`, subtle border hierarchy, and dot-matrix flow canvas).

2. **3-Zone Stage Telemetry Cards (`.stage-node`)**
   - Replaces flat stage squares with structured hardware/telemetry modules:
     - **Top Header Strip (`.stage-header`)**: Stage step index (`01`–`04`), role label (`VERIFY`, `SOAK`, `CANARY`, `RELEASE`), and high-contrast status badge (`PASSED`, `RUNNING`, `GATE`, `FAILED`, `LOCKED`, `LIVE`).
     - **Main Body (`.stage-body`)**: Bold stage title, elapsed/duration timer, and primary check summary.
     - **Progress & Telemetry Footer (`.stage-footer`)**: Embedded progress bar (`.stage-meter`) and dual sub-metadata labels.
     - **Hardware Terminal Ports (`::before` / `::after`)**: Left/right connection sockets vertically centered (`top: 50%`) to dock with incoming and outgoing promotion pipes.

3. **Executing Stage Loader Animation (`.stage-node.running`)**
   - **Rotary Spinner Ring (`.spinner-ring`)**: Animated SVG circular track + center core + rotating arc inside the `RUNNING` badge and active check rows.
   - **Animated Progress Meter**: Diagonal moving stripe fill (`@keyframes loader-stripes`) with a glowing leading edge and subtle pulsing border halo (`@keyframes stage-running-pulse`).

4. **Fluid Promotion Conduit Arrows (`.transition`)**
   - **Layered SVG Conduit**: Outer `.pipe-track`, inner `.pipe-base`, animated `.pipe-stream` (`stroke-dashoffset`), and three staggered glowing droplets (`.pipe-droplet`) sliding into a pulsing arrowhead (`@keyframes droplet-slide`, `@keyframes arrowhead-pulse`).
   - Supports distinct visual states for completed flow (`.done`), active/pending promotion flow (`.promoting` / `.next`), and gated/blocked paths (`.idle`).

## Application implementation

The [desktop interface](../apps/desktop/src/App.tsx) implements this visual
language for the real registry and includes an explicit **design preview** entry
in the top bar. The preview reuses the application's shell, stage cards, graph
layout, status icons, and keyboard-accessible tabs rather than embedding this
HTML study. It recreates the overview and release concepts with sample projects,
interactive state presets, simulated promotion, and a selected-stage inspector.

The following differences from the renders are intentional:

- Real manifest stages remain **not run**. Counts are planned checks, meters are
  unfilled, and approval policies never imply completed execution. The registry
  does not yet supply runtime versions, timers, built artifacts, or signatures.
  Reported harness evidence and manual approvals are labeled separately from
  execution telemetry.
- All running/passed/failed/live states, receipts, and terminal evidence are
  confined to the clearly labeled preview. Its status says **simulation only**,
  not **daemon online**, and no preview action calls the native registry or
  persists state.
- Stage roles in real graphs identify entries and all-parent joins, rather than
  guessing a role from a stage's name. Ports and conduits follow declared graph
  connections, including forks, independent endings, and vertical layouts.
- Cards retain readable dimensions and use local scrolling for large graphs.
  The inspector stacks on narrower windows, and animated elements respect
  `prefers-reduced-motion`.

See the [interface guide](../README.md#interface-and-design-preview) for usage.

## Consumer destinations

The application extends the stage modules with consumer badges and a destination
row, for example **beta -> Personal MacBook**. Selecting one opens the source
stage's consumer panel, with its destination, last manually recorded reference,
and **deployed to** action.

The confirmation form requires a version, commit, or artifact reference and an
explicit manual approval. Its history marker does not animate a promotion,
change stage status, or copy files. Real records are stored in the shared local
registry; names, destinations, and reviewed configuration metadata are retained
even after a manifest changes or an app is unregistered.

The React design preview uses the same destination panel, confirmation form,
and history renderer with isolated in-memory records. Its beta destinations
illustrate the interaction without calling the native bridge.

See the [consumer manifest guide](../README.md#stage-consumers-and-manual-deployment-history)
and [example manifest](../examples/consumers.yaml).

## Promotion criteria

Stages can declare promotion criteria with exact **n expected tests / m required
passes**, required consumer deliveries, and optional deliveries. The promotion
panel uses distinct satisfied, blocked, and warning states; a warning is not
silently treated as a completed step.

The approval review lists any optional steps being skipped and requires an
explicit acknowledgement. Its immutable history preserves the original
assessment and skipped steps even if later evidence changes. These approvals
record decisions only; real stage execution remains disabled.

The design preview demonstrates the same controls with isolated evidence and
approval history. Scenario presets remain illustrations rather than recorded
approvals. See the [policy guide](../README.md#promotion-criteria-and-reported-harness-results)
and [promotion example](../examples/promotion.yaml).
