### Run tests

    cargo test -p hotstuff-consensus --features testing --test e2e                        # all e2e tests
    cargo test -p hotstuff-consensus --features testing --test e2e simulator::scenarios   # simulator scenarios only
    cargo test -p hotstuff-consensus --features testing --test e2e progress_after_gst     # one test

The name filter matches a substring of the test path. The `e2e` target needs the `testing` feature
(`required-features` in `consensus/Cargo.toml`); without it cargo skips the target.

### Run with verbose traces

Set `VERBOSE` in `scenarios.rs` to `true` to trace every scenario, or enable it for one test only:

    setup(4).verbose(true).build()?

Run the test with `--nocapture` to see where the trace is written:

    cargo test -p hotstuff-consensus --features testing --test e2e synchronous_network_commits_without_timeouts -- --nocapture

Each simulator writes a markdown table `t | node | event | effects` to `target/tmp/traces/<test>.md`.
A test that builds several simulators, e.g. in a loop over seeds, gets `<test>.md`, `<test>-1.md`,
`<test>-2.md`, … in build order. A new run overwrites files with the same name and leaves the others, so
clear the directory to start fresh:

    rm -rf target/tmp/traces

Set `VERBOSE` back to `false` before committing.
