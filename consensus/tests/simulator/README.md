### Run tests

    cargo test -p hotstuff-consensus --features testing --test e2e                        # all e2e tests
    cargo test -p hotstuff-consensus --features testing --test e2e simulator::scenarios   # simulator scenarios only
    cargo test -p hotstuff-consensus --features testing --test e2e sync_happy_path        # one test

The name filter matches a substring of the test path. The `e2e` target needs the `testing` feature
(`required-features` in `consensus/Cargo.toml`); without it cargo skips the target.

### Traces

Scenarios run through `for_each_seed`, which passes `verbose = false`. When a seed fails, it reruns that
seed with `verbose = true`, so a failure gets a trace automatically, with the violation recorded in it.

To trace a specific scenario and seed, use `replay_seed`:

    #[test]
    fn replay_seed() -> Result<()> {
        let seed = 500;
        replay(seed, eviction)
    }

Pass the scenario function directly. `replay` enables tracing and uses its name, so this writes
`eviction-500.md` and `eviction-500.json` instead of `replay_seed-500`.

Run with `--nocapture` to see the trace path and the command to serve and open its viewer over HTTP:

    cargo test -p hotstuff-consensus --features testing --test e2e simulator::scenarios::replay_seed -- --exact --nocapture --test-threads=1

The trace is not printed to the console. Each simulator writes a markdown file to
`target/tmp/traces/<test>-<seed>.md`. Another simulator built with the same seed in the same test gets
`<test>-<seed>-1.md`, `<test>-<seed>-2.md`, … in build order. A new run overwrites files with the same
name and leaves the others, so clear the directory to start fresh:

    rm -rf target/tmp/traces

### Viewer

Open an existing JSON trace in the browser with one command:

    python3 docs/spec/quint/tools/viz.py target/tmp/traces/<test>-<seed>.json --serve

This serves a snapshot of the selected trace on `127.0.0.1`, chooses a free port, and opens the viewer
with the trace already loaded. Add `--step 170` to open a particular step, or `--port 8000` to choose
the port. Stop the server with `Ctrl+C`; rerun the command to load an updated trace.

Next to each `.md` goes a `.json` in the format of the spec trace viewer. Open it with "Open trace…" in
`docs/spec/quint/tools/viewer.html`, or inline it into one page:

    python3 docs/spec/quint/tools/viz.py target/tmp/traces/<test>-<seed>.json --html trace.html

`runner.mark(at, "label")` draws a labelled line across the diagram after the current step, and a row in the
`.md`. Each simulator event is one step. The page header shows the setup, the notes from `runner.note` and the
violation, if any; a failed trace opens at its last step, marked with a red line. A blocksync has no sender in the
simulator, so it is a `sync` tag on the step and not an arrow; the step label and the `Progress` card count the
blocks that were already stored. QCs have no signers, because a threshold signature does not reveal them.

Give each Byzantine handler a display name, for example
`ByzantineProxy::new("eviction", with_probability(70), eviction_behavior())`.
The lane header lists the handlers configured at the selected step; hover to see the full list.
`no handler` means the node is declared Byzantine but follows the protocol without an override.
The header describes configuration. Event points show the names of handlers actually called on that step,
with a caption and a `handlers` row in the event details. Byzantine calls have a red outline and the `faulty`
tag; mutation calls have an accent-colored outline and the `mutated` tag.
A handler may delegate to honest processing for an input; its name records the call, not necessarily an attack.

### Mutating checked nodes

Use `mutate_node` to override a node while keeping it in `honest_nodes()`, `honest_views()`,
and `honest_commit_logs()`. Safety and liveness checks continue to include it:

```rust
let adversary = Adversary::new([3])
    .mutate_node(NodeSelector::Id(NodeID::from(1)), || {
        ByzantineProxy::new(
            "silent",
            |_: &Context<'_, VirtualClock>| true,
            |_, _| Ok(no_effects()),
        )
    });
```

`Id` must name a non-Byzantine committee member. `All` selects all such members, and `Range(f)` selects
at most `f` of them using the scenario RNG. Selection includes nodes that have not started yet.
Each selected node gets its own handler state. Registrations are prepared once by `Runner::run`;
reusing the same `Env` preserves both the selection and handler state.

The viewer labels these nodes `mutated` and shows their handlers for the selected step. Mutation
registrations never add nodes to `Adversary::byzantine_nodes` or the viewer's faulty-node set.

`eviction_with_unlimited_votes_test` runs the eviction scenario with between `1` and `n - f`
randomly selected mutants. The `unlimited_votes_and_qcs` handler resets `voted_view` before each proposal,
including replayed proposals, so the node can vote repeatedly within a view. The usual view, lock,
and proposal validation checks still apply. The mutant count and identities come from the scenario seed.

The same mutants collect votes separately for each `(view, block)`, allowing several QCs for different
blocks in one view. Each collector keeps the normal quorum threshold and counts each signer only once
for its block. Vote verification, collector leadership, and buffering of future-view votes still apply.
Every assembled QC is replayed as `NewQC` through the normal driver, including blocksync if needed;
the mutation does not add a separate QC broadcast or change pacemaker handling of old QCs.

The scenario keeps the same safety and progress checks as `eviction`; violations fail the test and
produce a trace for inspection. To replay it, use `replay(seed, eviction_with_unlimited_votes)`.

`nonconsecutive_commit()` disables only the requirement that the two QCs used for a commit have
consecutive views. It keeps normal voting, lock updates, block validation, and the check that a commit
extends the executed chain. The handler lives entirely in the simulator: after normal processing of
a validated proposal or synced block, it applies the relaxed commit rule to the stored chain and adds
any additional commits to storage and the returned effects. Buffered proposals are handled on replay:

```rust
let adversary = Adversary::new([3])
    .byzantine_node(NodeSelector::All, || {
        ByzantineProxy::new(
            "tail_fork",
            |_: &Context<'_, VirtualClock>| true,
            tail_fork(),
        )
    })
    .mutate_node(NodeSelector::Id(NodeID::from(1)), || {
        ByzantineProxy::new(
            "nonconsecutive_commit",
            |_: &Context<'_, VirtualClock>| true,
            nonconsecutive_commit(),
        )
    });
```

The existing `tail_fork()` handler replaces a leader's proposal with a block built on its locked QC
when that QC is older than the proposal's justify. This can exercise view gaps; conflicting commits
still require an appropriate message schedule and competing branches.
