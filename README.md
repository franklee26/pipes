# Pipes

A local release manager with a Rust core, a Tauri desktop interface, and a CLI.

## Current capabilities

- Execute explicitly approved pipelines with a shared Rust runner in the desktop and CLI.
- Stream live stdout/stderr with stage and command attribution; retain run status and logs.
- Extract command status from configurable exit codes or a user-defined JSON result file.
- Preview and validate YAML manifests without executing their commands.
- Explicitly register applications in a persistent local SQLite registry.
- Inspect saved manifest snapshots and their configured stage paths.
- Display forks, all-parent joins, and independent branch endings.
- Describe per-stage sequential and parallel check groups, with shared concurrency budgets.
- Declare per-stage consumer destinations and manually approve deployment history markers.
- Retain the original destination and release reference in immutable deployment history.
- Define exact test-count thresholds and required or optional deployment criteria.
- Record externally produced harness results and assess candidate-specific promotion readiness.
- Save promotion-criteria approvals with explicitly acknowledged optional skips.
- Replace a registration only with explicit confirmation.
- Unregister applications without deleting source files, manifests, or deployment history.
- Use the same validation and persistence implementation from the UI and CLI.

The runner executes arbitrary, noninteractive command argument arrays. There are
no framework-specific test adapters: your program defines its result contract.
Registering or inspecting a manifest still never executes it.

This first runner uses an explicitly opted-in **working tree** and is attached
to its owning desktop app or CLI. Git source snapshots, built-in artifact storage,
fetching, rollback, and signing are not implemented. Commands may build or test
real projects, but their output is not an artifact or source attestation.
Consumer deployment records still acknowledge a manual action; they do not
execute or verify deployment. Harness-count reports remain externally supplied
evidence and are not inferred from command output.

## Development

Requirements:

- Node.js 24 and npm.
- Rust via rustup. [rust-toolchain.toml](rust-toolchain.toml) pins the toolchain.
- The [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/)
  for your platform. On macOS, install Xcode and its command-line tools.

From the repository root:

```sh
npm ci
npm run dev
```

`npm run dev` starts Vite and opens the native desktop window. In the window,
select **+ manifest**, browse to [examples/pipes.yaml](examples/pipes.yaml),
review the source and planned commands, then select **register**. Open the
registered application and choose **run pipeline** to review and explicitly
approve an execution. Registration alone does not start it.

If Node 24 was installed using Homebrew:

```sh
export PATH="$(brew --prefix node@24)/bin:$PATH"
```

The VS Code tasks include this Homebrew path on macOS. Tasks are provided for
desktop development, desktop builds, frontend builds, type checking, and UI tests.

The frontend alone can be started with:

```sh
npm run dev --workspace apps/desktop
```

A regular browser has no native registry connection and explicitly reports that
limitation. It never substitutes sample data or browser storage for the registry.
Select **design preview** to explicitly enter a separate, simulated workspace
that also works in a browser.

### Interface and design preview

The interface follows the [Concept v3 design study](design/README.md): cream
surfaces, a deep-green navigation rail, monospace typography, three-part stage
telemetry cards, and a dot-matrix pipeline canvas.

- **Pipelines** shows registered manifests and their actual configured graph.
  Select a stage to inspect its ordered/parallel check plan, requirements, saved
  manifest, declared outputs, and consumer destinations alongside the stage
  inspector. Consumer badges and the destination row identify which stage feeds
  each device or installation.
- A stage's **promotion** tab shows its exact manifest thresholds, reported
  harness evidence, readiness blockers and warnings, and frozen approval history.
- Real runs show live stage/check status, elapsed time, and terminal output.
  Follow output as it arrives, or pause following to inspect earlier messages.
  Cancel an active run or explicitly approve a waiting local stage gate.
- **Manifest** opens a library of saved snapshots, including source paths and
  SHA-256 digests. **Storage** shows the actual registry location and counts.
- **Releases** includes retained pipeline runs and their original manifest,
  alongside deployment markers, externally reported harness results, and
  promotion-criteria approvals, including unregistered applications.

**Design preview** is deliberately separate from the registry. Its persistent
preview banner identifies every screen as simulated. Explore the Atlas, Beacon,
and Relay examples from the study, select stages, switch detail tabs, and use
Atlas's presets to see executing, approval-gate, and production-promotion states.
The preview arrows and action buttons advance only the simulation. Artifact
names, receipts, history, and terminal output are samples, not stored evidence.
No commands run, files download, registrations change, or preview state persists.
The sample beta consumers also support **deployed to** approvals; their markers
are kept in memory only and reset on exit.
**Exit preview** returns to the real workspace.

Pipeline cards retain readable text and scroll when a graph is larger than its
panel. Narrow panels switch to a vertical layout, with connections and ports
following the real forks and all-parent joins. Keyboard navigation is supported
for stages and detail tabs. System reduced-motion preferences disable the
spinners, meter stripes, and flowing-pipe animations without hiding status.

## CLI

Run the CLI directly from the workspace:

```sh
cargo run --locked -p pipes-cli -- validate examples/pipes.yaml
cargo run --locked -p pipes-cli -- register examples/pipes.yaml
cargo run --locked -p pipes-cli -- list
cargo run --locked -p pipes-cli -- inspect pipes
```

Run a registered pipeline with explicit execution consent and a candidate label:

```sh
cargo run --locked -p pipes-cli -- run pipes --release-ref worktree-test-1 --approve
```

Output streams live while this command remains attached. In another terminal,
inspect progress, approve a local gate, or cancel:

```sh
cargo run --locked -p pipes-cli -- runs pipes
cargo run --locked -p pipes-cli -- run-status RUN_ID
cargo run --locked -p pipes-cli -- logs RUN_ID
cargo run --locked -p pipes-cli -- approve-stage RUN_ID --stage gamma --approve
cargo run --locked -p pipes-cli -- cancel-run RUN_ID
```

`run` accepts the same paired review tokens described below. Each invocation
creates a new run; it never silently reruns or resumes an old one. Failed,
cancelled, and interrupted runs return a nonzero exit status. With `--json run`,
stdout is one final run snapshot; live stderr is newline-delimited JSON output
events (`event`, `runId`, `entry`). `--json logs RUN_ID --after CURSOR` returns a
bounded page with `entries`, `nextCursor`, and `hasMore`. Follow the returned
cursor to retrieve all output. `runs` returns the newest 100 matching runs.

Register an edited manifest with explicit replacement:

```sh
cargo run --locked -p pipes-cli -- register examples/pipes.yaml --replace
```

Unregister application metadata only; existing deployment history is retained:

```sh
cargo run --locked -p pipes-cli -- unregister pipes
```

Unregistration removes metadata only. It does not cancel an active run; use
`cancel-run` first when that is your intent. Runs remain inspectable and
cancellable through their retained history after the app is unregistered.

Record an explicitly approved manual consumer deployment:

```sh
cargo run --locked -p pipes-cli -- register examples/consumers.yaml
cargo run --locked -p pipes-cli -- deployed-to pipes-consumers \
  --stage beta --consumer personal-mac --release-ref v0.1.0 --approve \
  --note "Copied the beta build to my laptop."
cargo run --locked -p pipes-cli -- deployments pipes-consumers
cargo run --locked -p pipes-cli -- --json deployments
```

`--approve` is required. The CLI captures the current saved manifest hash and
registration revision before submitting the record; concurrent changes cause
rejection. For a previously reviewed registration, pass **both**
`--expected-hash HASH` and `--expected-updated-at TIMESTAMP` from `inspect`.
The hash alone is insufficient because relocating an identical manifest can
change its resolved source without changing its YAML.

`deployments` with no app ID lists all markers. An app-filtered query still works
after that app is unregistered. Each explicit approval appends a new marker;
repeated deployments of the same reference are not collapsed.

Report external results and approve a promotion policy:

```sh
cargo run --locked -p pipes-cli -- register examples/promotion.yaml
cargo run --locked -p pipes-cli -- report-harness pipes-promotion \
  --stage beta --check smoke --release-ref v0.1.0 --total 50 --passed 45
# After manually delivering this candidate to the required lab destination:
cargo run --locked -p pipes-cli -- deployed-to pipes-promotion \
  --stage beta --consumer lab-device --release-ref v0.1.0 --approve
cargo run --locked -p pipes-cli -- --json promotion-status pipes-promotion \
  --stage beta --release-ref v0.1.0
cargo run --locked -p pipes-cli -- approve-promotion pipes-promotion \
  --stage beta --release-ref v0.1.0 --approve --acknowledge-optional
cargo run --locked -p pipes-cli -- harness-results pipes-promotion
cargo run --locked -p pipes-cli -- promotion-approvals pipes-promotion
```

The sample command and counts illustrate a user-owned harness; customize them
for your real suite. `report-harness` records supplied totals, not counts extracted
by the runner. `promotion-status` returns a valid
assessment even when blocked; its exit status indicates whether assessment
succeeded, so automation must inspect the returned `status`. `approve-promotion`
is the enforcing operation: blockers reject it, and optional warnings require
`--acknowledge-optional`.

The new reporting/status/approval commands accept the same paired
`--expected-hash` and `--expected-updated-at` review tokens. Approval also accepts
`--expected-assessment-hash` from a previously reviewed assessment; otherwise the
CLI obtains one before attempting the transaction. New evidence invalidates an
old fingerprint rather than silently approving a different decision.

Use `--json` for machine-readable output:

```sh
cargo run --locked -p pipes-cli -- --json list
```

Command results go to stdout. Invocation and API failures use a nonzero exit
status and write `{"error":{"code":"...","message":"..."}}` to stderr in JSON mode.
Terminal execution failures are different: `--json run` returns the final failed,
cancelled, or interrupted run snapshot on stdout, with NDJSON output events on
stderr and a nonzero exit status.

Build or install the executable:

```sh
cargo build --locked -p pipes-cli
cargo install --locked --path crates/pipes-cli
pipes --help
```

## Local state

By default, Pipes uses the operating system's local application data directory.
The desktop's **storage** view displays the resolved directory and database path.

Set `PIPES_DATA_DIR` to isolate a workspace or use a different storage location.
It applies to both the desktop and CLI. The CLI also accepts `--data-dir`.

```sh
PIPES_DATA_DIR="$PWD/.pipes-dev" npm run dev
cargo run --locked -p pipes-cli -- --data-dir .pipes-dev register examples/pipes.yaml
```

The registry contains an exact YAML snapshot, its SHA-256 digest, resolved paths,
parsed planning metadata, timestamps, pipeline-run snapshots and append-only
output, and separate deployment, harness-result, and promotion-approval histories.
Run snapshots retain their original configuration even after replacement or
unregistration. Inspecting saved configuration or history does not require the
original manifest file. Active run views refresh live; returning focus to the
desktop also refreshes the registry.

Identical registrations are idempotent. Different content, a different manifest
path, or a changed resolved source path under an existing app ID requires
replacement. A manifest path
cannot silently become a different registered application.

The UI sends the reviewed digest back during registration. If the file changed
since preview, registration fails and requires another review.

Recording a consumer deployment checks both the reviewed manifest hash and
registration `updatedAt` under the same transaction that writes the marker.
The backend resolves the consumer from that saved snapshot, not from destination
details supplied by a client. Changed, removed, or unregistered destinations
require a fresh review. Neither recording nor reading history requires the
original source directory or manifest file to remain on disk.

Harness reporting uses the same registration review tokens. Promotion assessment
reads a consistent snapshot; approval re-evaluates the latest evidence inside
its write transaction and checks the reviewed assessment fingerprint. Neither
operation reads reports or source files, runs a harness, or contacts a consumer.

SQLite transactions, uniqueness constraints, WAL, and a busy timeout coordinate
the clients. OS-backed source locks prevent simultaneous runs against the same
canonical source directory within one registry. Different data directories are
independent. There is no background daemon: normal owner shutdown stops its
process groups and records interruption. Lost-owner runs are recovered as
**interrupted**, never as passed, and never automatically resumed.

## Pipeline execution

### Explicit working-tree execution

Opt in before starting a real run:

```yaml
source:
  path: ..
  snapshot: working-tree
  require_clean: false
```

The compatibility default remains `snapshot: git-commit`. Such manifests still
validate and register, but execution is rejected until Git snapshots are
implemented or you explicitly choose `working-tree` and re-register. The runner
does not silently substitute a working tree for a requested snapshot.

`require_clean` defaults to `true`. It checks Git's tracked and untracked changes
at launch; non-Git or deliberately dirty projects must explicitly set it to
`false`. This is a preflight check, not isolation from subsequent source edits.
The Git check is logged, cancellable, and limited to 30 seconds; failures retain
a failed run without executing any manifest commands.
Host OS/architecture constraints are also checked before starting. The candidate
reference is entered by the user; it is not read from Git or `version.from`.

Starting requires confirmation of the saved manifest hash and registration
revision, source path, and commands. Editing the original YAML does not mutate
a saved registration or a running pipeline. Re-register changes before using
them in a new run.

### Framework-agnostic result contracts

Every command leaf, including preparation commands, may declare `result`.
Omitting it is equivalent to:

```yaml
result:
  type: exit-code
  success_codes: [0]
```

List one or more distinct success codes from 0 through 255. Other normal exit
codes fail. Pipes does not interpret framework-specific console output.

For a user-owned wrapper that writes JSON:

```yaml
- id: custom-suite
  exec: [node, scripts/run-suite.cjs]
  result:
    type: json-file
    path: reports/pipes-status.json
    pointer: /execution/status
    success: passed
    failure: failed
```

The program writes, for example:

```json
{"execution":{"status":"passed"}}
```

- `path` is absolute or relative to the command's actual working directory.
  Pipes provides its absolute value as `PIPES_RESULT_FILE`; your program must
  create any parent directories and write the file. No shell or environment
  expansion is performed on manifest strings.
- `pointer` is an RFC 6901 JSON Pointer, with `~0` for `~` and `~1` for `/`.
  An empty pointer selects the whole JSON value.
- `success` and `failure` are required, distinct, non-null scalar JSON values:
  strings, booleans, or numbers. Values are compared without string coercion.
  Prefer strings for very large integer statuses: the UI directs you to the
  original YAML when a number exceeds JavaScript's exact integer range.
- Results are read **after normal process exit**. In JSON-file mode the file is
  authoritative even for a nonzero normal exit code. Signals, timeouts, and
  cancellation cannot be converted into success by a result file.
- The file must be newly created or observably changed during that command.
  An unchanged old result, missing/nonregular/symlink file, malformed or oversized
  JSON, missing pointer, or unrecognized value fails explicitly. Files are bounded
  to 1 MiB and are never deleted by the runner. Always rewrite the complete result
  for each execution; do not touch an old success file to reuse its status.
- Checks sharing a result path are serialized so they cannot concurrently
  overwrite each other's status. Prefer a distinct result file for each check.
- This contract records command status only. It does not automatically produce
  harness-count evidence or fulfill an exact n/m promotion criterion.

See [examples/runner.yaml](examples/runner.yaml) for real command execution and
a small user-owned JSON-status adapter.

### Scheduling, approval, and process lifetime

Preparation commands run once in declaration order before stages; every
preparation must pass. Entry stages then become eligible independently. Stage
dependencies are all-parent barriers, including forward references and forks.
Check lists and `sequence` groups preserve order; `parallel` groups share the
run-wide and ancestor concurrency budgets. Every command leaf is required.

A failed command skips its ordered successors. Other eligible parallel branches
are allowed to finish, while stages depending on a failed stage are blocked.
Independent branch endings continue. The pipeline passes only when every
required command and stage passes.

`approval.local` pauses **before** a stage's commands execute. Approve that stage
for that specific run in the UI or with `approve-stage`; initial execution consent
does not approve future gates. Configured `promotion.criteria` pause stage
completion **after** its commands pass until a currently matching promotion
assessment is explicitly approved. Use the existing promotion tab or
`report-harness`, `deployed-to`, and `approve-promotion` workflow for the same
candidate and registration. Missing evidence, stale approval fingerprints, or
changed registrations never silently unlock that gate.

`exec` is an argv array, not a shell string. Use an explicit shell command only
when you intend shell behavior. `cwd` is relative to the registered source unless
absolute. Commands inherit the owner's environment plus declared `env` entries.
Desktop apps launched outside a configured shell may have a smaller `PATH`; use
absolute executable paths or an explicit `env.PATH` for project-specific tools.
Manifest environment values are literal strings, not shell substitutions.
`env://` secret references are supported; missing/empty values and unsupported
`keychain://` providers fail before execution. Declared secret values are redacted
from captured output, including values split across chunks; this is not a
guarantee against arbitrary secret disclosure by your program.

The runner reserves `PIPES_RUN_ID`, `PIPES_RELEASE_REF`, `PIPES_STAGE_ID`,
`PIPES_COMMAND_ID`, and `PIPES_RESULT_FILE`; command `env` and `secrets` cannot
override them. Preparation commands use `PIPES_STAGE_ID=prepare`.

Command `timeout` overrides `execution.timeout`. Both are per-command wall-clock
limits; without either, a command runs until it exits or is cancelled. Timeouts
and cancellation stop the command's process group, not just its immediate shell.
Normal desktop shutdown interrupts owned runs; Ctrl-C cancels an attached CLI
run. Commands must not daemonize or detach descendants. Forced process termination
or detached descendants cannot be guaranteed graceful cleanup; saved unfinished
runs are marked interrupted on recovery rather than resumed.

### Live terminal output

The desktop follows persisted stdout/stderr incrementally, with stage and command
labels, real status, and safe terminal-style rendering. Output arriving without a
newline is displayed without waiting for command completion. Use follow controls
and filters to inspect interleaved parallel output. History is available after
reload or unregistration, with cursor-based paging rather than a lossy event-only
subscription.

These are noninteractive pipes, not a PTY: stdin is closed, and some tools buffer
output or suppress terminal progress when redirected. Configure your program to
flush progress (for example `python -u` or a framework's noninteractive reporter)
as needed. Pipes cannot display bytes a program has not emitted. Invalid UTF-8
is replaced for display. Captured stdout/stderr is limited to 64 MiB per run;
an explicit truncation marker is retained and excess output is drained so a noisy
process cannot block. The UI keeps a bounded visible tail.

## Manifest

See [examples/pipes.yaml](examples/pipes.yaml) for a complete registration example.

```yaml
api: pipes/v1
app: my-app
name: My app
source:
  path: .
  snapshot: working-tree
  require_clean: false
version:
  scheme: semver
  from: VERSION
stages:
  - id: test
    checks:
      - id: unit
        exec: [cargo, test, --locked]
    require: [checks.passed]
  - id: prod
    require: [previous.passed, approval.local]
```

### Forks and joins

Use `after` to name a stage's immediate predecessors. For example:

```yaml
stages:
  - id: test
    after: []
  - id: linux
    after: [test]
    require: [previous.passed]
  - id: macos
    after: [test]
    require: [previous.passed]
  - id: docs
    after: [test]
    require: [previous.passed]
  - id: prod
    after: [linux, macos]
    require: [previous.passed, approval.local]
```

This branches from `test` into `linux`, `macos`, and `docs`. The `prod` stage joins
`linux` and `macos`; `docs` ends independently. Joins require **all** incoming
parents, not whichever finishes first. Registration displays the graph without
executing it; an explicitly started run enforces these dependencies.

- If `after` is omitted, the predecessor is the previous listed stage. The first
  stage has no predecessor. Existing linear manifests keep their meaning.
- Explicit `after: []` declares an entry stage, regardless of its list position.
- Forward references, multiple entry stages, and multiple terminal stages are
  supported. The UI uses graph connections, not list adjacency, to draw arrows.
- Unknown or duplicate predecessors, self-links, `after: null`, and cycles are
  rejected.
- The graph keeps labels at a readable size and scrolls when it does not fit.
  Narrow panels use a top-to-bottom layout.

See [examples/forks.yaml](examples/forks.yaml) for a complete manifest that includes
both a rejoin and an independently ending branch.

### Stage consumers and manual deployment history

A **consumer** is a named destination fed by a stage, such as a personal device
receiving a beta build. Define it on the source stage:

```yaml
stages:
  - id: beta
    consumers:
      - id: personal-mac
        name: Personal MacBook
        target: "My MacBook / Applications/MyApp.app"
        approval: manual
  - id: prod
    after: [beta]
    require: [previous.passed, approval.local]
```

See [examples/consumers.yaml](examples/consumers.yaml) for a complete example.
Consumers are destinations, **not additional executable stages**. They do not
become `after` nodes or alter forks, joins, or check execution. A promotion policy
can require a consumer's manual deployment marker or treat it as an optional step.

- Consumer IDs are lowercase ASCII slugs and must be unique within their
  source stage. The same ID can be used in a different stage.
- `name` is optional and defaults to the ID. Names are nonempty, at most 128
  Unicode characters, and cannot contain control characters.
- `target` is a required, nonempty description of where the app was sent, at
  most 1024 Unicode characters without control characters. It is display-only:
  Pipes never resolves it as a path, contacts a device, or accesses credentials.
  Use destination labels or paths, not passwords or credential values.
- `approval` defaults to `manual`, the only supported value. It cannot be
  disabled or set to automatic. Omitted `consumers` means none; `null` is invalid.

In the desktop app:

1. Register the manifest, or re-read and explicitly replace its registration
   after editing consumer definitions.
2. Select the source stage or its consumer destination row.
3. Choose **deployed to**, enter a version, commit, or artifact reference, and
   optionally a note.
4. Confirm that you manually delivered that reference to the displayed
   destination, then choose **approve & record**.

The reference is required, trimmed, and limited to 256 Unicode characters with
no control characters. Notes are optional, trimmed, and limited to 4096 Unicode
characters; newlines and tabs are allowed, other control characters are not.
No reference is read from `VERSION`, Git, or an artifact store. A stage need not
have run to record an external manual deployment, and recording one does **not**
mark that stage passed, satisfy `approval.local`, or advance the pipeline.

The selected stage's **deployments** tab shows its markers; **Releases** shows
the whole history. Each entry retains the app name and ID, source stage, original
consumer name and target, entered reference, optional note, reviewed manifest
hash and paths, registration revision, and server-generated UTC recording time.
The time describes when the marker was recorded, not a verified deployment time.
The manifest hash identifies reviewed configuration, not verified artifact bytes.

History is immutable through the UI and CLI. Editing or removing a consumer,
replacing a manifest, or unregistering an app does not rewrite or erase earlier
entries. A changed destination does not inherit the previous destination's
latest-deployment indicator. Record another marker (and a note) for a later
deployment, rollback, or correction; old entries remain available.

### Promotion criteria and reported harness results

Put a `promotion.criteria` list on the **source stage** whose promotion decision
you want to assess:

```yaml
promotion:
  criteria:
    - id: smoke-tests
      kind: harness
      check: smoke
      expected_tests: 50
      minimum_passed: 45
    - id: lab-delivery
      kind: deployment
      consumer: lab-device
      requirement: required
    - id: personal-delivery
      kind: deployment
      consumer: personal-device
      requirement: optional
```

See [examples/promotion.yaml](examples/promotion.yaml) for a complete manifest.
Each criterion has a unique slug ID within its stage. Harness references must
name command leaves in that stage's check plan, not a group, preparation command,
or another stage. Deployment references must name a consumer of that stage.
Duplicate references, unknown fields, unsupported kinds, nulls, and an explicitly
empty criteria list are rejected. Omitting `promotion` means **not configured**,
not an unconditional approval.

#### Exact n/m harness assertions

The manifest asserts **n expected tests** and **m required passes**, with both
fields explicit and `1 <= m <= n <= 4294967295`. Success requires:

```text
reported total == expected_tests
AND reported passed >= minimum_passed
```

For `n = 50`, `m = 45`:

| Report | Criterion |
|---|---|
| 44 passed / 50 total | Blocked: below the minimum |
| 45 passed / 50 total | Satisfied |
| 50 passed / 50 total | Satisfied |
| 45 passed / 49 total | Blocked: incomplete or changed suite |
| 50 passed / 51 total | Blocked: unexpected suite size |
| No report | Blocked: missing evidence |

Use `minimum_passed: 50` with `expected_tests: 50` to require every test to pass.
There is no percentage rounding or inferred test count. Reports contain observed
whole-number totals and passes, including zero/zero when a harness discovers no
tests. Zero/zero is valid evidence to retain, but cannot satisfy a positive-n
policy. Passed tests cannot exceed the reported total. The reporter must include
all reported outcomes in the total, including failed or skipped cases.

Only the **latest matching report** is used. A newer failed or incomplete report
does not fall back to an older passing result.

#### Required and optional deployments

A `required` deployment criterion blocks approval until a matching manual
deployment marker exists. An `optional` missing deployment produces a warning,
not a blocker. If all required criteria are satisfied, the policy is eligible
with warnings; approving it requires acknowledging the listed optional skips.

Evidence must match the candidate's exact release reference and the current
registration's manifest hash, paths, and revision. Deployment evidence must also
match the consumer ID and destination. Results for another candidate, an older
configuration, a relocated source, or a different device cannot unlock a gate.
After a configuration change, report or acknowledge evidence against the new
registration; older records remain in history.

The desktop's **promotion** tab lets you report actual harness counts, enter a
candidate reference, and **assess readiness**. Missing or failed required
criteria disable approval. Optional misses are highlighted, and **review
optional skips & approve** opens a confirmation that requires both approval and
explicit acknowledgement of the skips.

An approval stores the complete original policy assessment, observed counts,
chosen evidence IDs, destination descriptions, and skipped optional criterion
IDs. Later delivery of an optional step does not rewrite an earlier decision.
Approvals and harness reports survive configuration changes and unregistration.
The backend rejects approval if relevant evidence or the reviewed registration
changed while the confirmation was open.

**Scope:** the policy service assesses and approves only `promotion.criteria`.
The runner separately enforces `checks.passed`, `previous.passed`,
`prepare.*.passed`, and `approval.local`. A policy approval does not attest those
conditions or itself execute commands. An active runner observes a currently
matching approval to release its custom-policy gate only after command checks
have passed. Externally reported counts and manual deployment markers remain
caller-supplied evidence, not artifacts or test counts verified by Pipes.

### Sequential and parallel check sets

A stage's `checks` list runs **in order**. An entry can be an ordinary command, a
named `parallel` group, or a named `sequence` group:

```yaml
execution:
  max_parallel: auto

stages:
  - id: beta
    checks:
      - id: first-set
        parallel:
          - id: unit
            exec: [cargo, test, --locked]
          - id: typecheck
            exec: [npm, run, check]

      - id: second-set
        max_parallel: 2
        parallel:
          - id: interface
            exec: [npm, test]
          - id: packaging
            sequence:
              - id: build
                exec: [npm, run, build:web]
              - id: inspect
                exec: [pipes, validate, examples/pipes.yaml]
    require: [checks.passed]
```

The entire first set must pass **before** any check in the second set is eligible.
Within the second set, `interface` can overlap the `packaging` branch, but `inspect`
waits for `build`. Every check is required; `checks.passed` covers the complete
tree, not just one batch.

Registration, preview, and inspection never execute these plans. An explicitly
approved pipeline run enforces their ordering and budgets.

#### Command budgets

- `execution.max_parallel` is a shared pipeline/run command limit. It defaults to
  `auto`, meaning the host's available capacity at execution time. Registration
  does not resolve or freeze a machine-specific CPU count.
- A positive integer overrides the auto budget.
- A `parallel` group's optional `max_parallel` adds a ceiling for that group and
  all descendants. It cannot raise a parent or run limit. Nested parallel groups
  share limits; their capacities are not multiplied.
- The effective finite ceiling is the smallest run/ancestor/group cap. With an
  auto run budget, host capacity is also a ceiling.
- These limits count **simultaneous commands**, not threads or subprocesses
  created internally by pytest, Cargo, Xcode, or other tools. Tool-specific worker
  settings remain explicit command arguments or environment variables.

Group IDs and command IDs share one unique namespace within a stage. Groups
must be nonempty; single-child groups are valid. Use exactly one of `exec`,
`parallel`, or `sequence` per entry. Command options such as `cwd`, `env`,
`secrets`, and `timeout` belong to command leaves, not groups. Group nesting is
limited to 32 levels.

The UI preserves group boundaries, displays `ALL PASS / THEN` barriers, and shows
effective ceilings. It does not flatten parallel and sequential plans into an
execution order. In JSON output, `checkPlan` carries the full structure;
`checks` remains a flattened, declaration-order list for inspection and counting.

See [examples/check-sequences.yaml](examples/check-sequences.yaml) for a complete
example combining ordered sets, concurrent checks, an ordered branch, and a
per-group cap.

### Saved-registry compatibility

Opening a version-1 through version-5 registry upgrades saved metadata
transactionally to version 6. Legacy flat checks become an ordered plan of command
leaves, with default concurrency `auto`. Existing stage connections, YAML, hashes,
resolved paths, check plans, budgets, consumers, and registration timestamps are
preserved. Existing version-4 consumer history keeps its records and IDs.
Stages predating custom policies gain empty promotion-policy lists. Existing
version-5 reports and approvals are preserved, commands without a result contract
retain exit-code-zero semantics, and the new run/output histories start empty.
Original source and manifest files are not needed.
Invalid snapshots or incompatible history schemas abort the entire upgrade
rather than being reset or silently repaired. Older Pipes builds cannot read the
upgraded registry.

### Validation rules

- YAML files must be regular UTF-8 files, at most 1 MiB.
- YAML parsing/expansion is also bounded to 1,048,576 values, 8 MiB of scalar
  text, and 96 collection levels. Check groups have the stricter 32-level limit.
- The API version is `pipes/v1`; unknown fields and unsupported values are errors.
- Application, stage, consumer, criterion, and command identifiers use lowercase ASCII slugs.
- At least one stage is required. Stage IDs must be unique. Command and group IDs
  must be unique throughout each stage's check tree.
- `source.path` is resolved relative to the canonical manifest's directory,
  unless absolute. The source directory must exist.
- `version.from` records a source-relative version-file path. This foundation
  does not read or evaluate that version file.
- Commands use a nonempty `exec` argument array. Optional `cwd`, `timeout`,
  `env`, `secrets`, and `result` are validated without executing them at registration.
- Concurrency caps are positive integers up to `4294967295`. `auto` is allowed
  only for the run-level budget; omitting a group cap inherits its shared budget.
- Durations must be positive, for example `30s` or `20m`.
- `secrets` contains credential references such as `env://SIGNING_IDENTITY`
  or `keychain://pipes/signing`, not credentials themselves.

Optional top-level fields:

| Field | Purpose |
|---|---|
| `execution` | Host OS/architecture, default timeout, and shared command budget |
| `prepare` | Named preparation commands |
| `outputs` | Named planned file or directory outputs |
| `retention` | `keep-all` intent and protected-reference categories |

Supported condition declarations:

- `checks.passed`: the stage must contain at least one command leaf; its entire
  sequential/parallel plan must pass.
- `previous.passed`: all immediate predecessors must pass; valid only when the
  stage has at least one resolved predecessor.
- `approval.local`: explicit per-run approval before stage commands execute.
- `prepare.<id>.passed`: references a declared preparation step.

Source snapshots, clean-tree requirements, tools, version files, credential
availability, and actual command outcomes are not checked at registration.
No Git or manifest command is spawned at registration. Registering a manifest
does not grant execution approval. Execution preflight checks supported source
and host policies; artifact/signing/version-file capabilities remain out of scope.

## Validation and builds

```sh
npm run check
npm test
npm run build:web
cargo test --locked --workspace
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Build the desktop application:

```sh
npm run build
```

On macOS, build just the application bundle without a disk image:

```sh
npm run build -- --bundles app
```

Core and CLI tests use temporary registries. Native IPC tests exercise the real
command handlers against isolated registries, including reviewed-hash checks and
remote-origin rejection. Frontend tests use an injected, test-only API; production
code always calls the native Rust bridge.

For core and CLI testing without desktop system dependencies:

```sh
cargo test --locked -p pipes-core -p pipes-cli
```

## Layout

- [crates/pipes-core](crates/pipes-core): manifest validation, registry, shared runner, and result contracts.
- [crates/pipes-cli](crates/pipes-cli): the `pipes` command.
- [apps/desktop/src](apps/desktop/src): the React interface.
- [apps/desktop/src-tauri](apps/desktop/src-tauri): native dialogs and typed IPC adapters.

The native interface exposes registration, reviewed pipeline execution, paged run
output, cancellation, stage approval, external result reporting, criteria
assessment/approval, and manual deployment-record operations. It has no generic
shell command or remote UI content: execution is limited to explicitly approved
commands from saved manifests.
