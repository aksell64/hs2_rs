### Run tests

    cargo test -p hotstuff-consensus --features testing --test e2e                        # all e2e tests
    cargo test -p hotstuff-consensus --features testing --test e2e simulator::scenarios   # simulator scenarios only
    cargo test -p hotstuff-consensus --features testing --test e2e sync_happy_path        # one test

The name filter matches a substring of the test path. The `e2e` target needs the `testing` feature
(`required-features` in `consensus/Cargo.toml`); without it cargo skips the target.

### Traces

Scenarios run through `for_each_seed`, which passes `verbose = false`. When a seed fails, it reruns that
seed with `verbose = true`, so a failure gets a trace automatically, with the violation recorded in it.

To trace a passing seed, temporarily change the test:

    let seeds = random_seeds(1);   // or a specific seed: let seeds = [16294208416658607535];
    .verbose(true)                 // instead of .verbose(verbose)

`VERBOSE` in `scenarios.rs` does not help here, because `.verbose(verbose)` in the test overrides it.

Run the test with `--nocapture` to see where the trace is written:

    cargo test -p hotstuff-consensus --features testing --test e2e sync_after_gst_tc -- --nocapture

The trace is not printed to the console. Each simulator writes a markdown file to
`target/tmp/traces/<test>-<seed>.md`. Another simulator built with the same seed in the same test gets
`<test>-<seed>-1.md`, `<test>-<seed>-2.md`, … in build order. A new run overwrites files with the same
name and leaves the others, so clear the directory to start fresh:

    rm -rf target/tmp/traces

Revert the temporary changes before committing.
