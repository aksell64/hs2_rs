use super::adversary::*;
use super::dsl::*;
use super::invariants::*;
use super::rng::SharedRng;
use super::runner::{Env, Runner};
use anyhow::{Result, ensure};
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};

const DELTA: u64 = 10;
const VIEW_TIMEOUT: u64 = 100;
const LOCK_TIMEOUT: u64 = 30;
const VERBOSE: bool = false;

const N4: u32 = 4;

#[test]
fn sync_happy_path() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, |seed, verbose| {
        const END: u64 = 2_000;
        let rnd = SharedRng::new(seed);
        let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
            .rng(rnd.clone())
            .verbose(verbose)
            .build()?;

        let mut runner = Runner::new(setup);
        runner.run(
            Env::new()
                .invariant(safety())
                .invariant(no_k_view_timeouts(1))
                .stop_cond(time_reached(END)),
        )?;

        let min_commited_height = min_committed_height(runner.state());
        let want_commited_height = min_view(runner.state()) - 2;
        ensure!(
            min_commited_height >= want_commited_height,
            "committed up to {min_commited_height}, expected at least {want_commited_height}"
        );

        Ok(())
    })
}

#[test]
fn sync_uneven_start_no_byzantine_no_adversary() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, |seed, verbose| {
        const END: u64 = 2_000;
        let rnd = SharedRng::new(seed);
        let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
            .rng(rnd.clone())
            .start_in_interval(&rnd, 0, DELTA - 1)
            .verbose(verbose)
            .build()?;

        let mut runner = Runner::new(setup);
        runner.run(
            Env::new()
                .invariant(safety())
                .invariant(no_k_view_timeouts(1))
                .stop_cond(time_reached(END)),
        )?;

        let min_commited_height = min_committed_height(runner.state());
        let want_commited_height = min_view(runner.state()) - 2;
        ensure!(
            min_commited_height >= want_commited_height,
            "committed up to {min_commited_height}, expected at least {want_commited_height}"
        );

        Ok(())
    })
}

#[test]
fn sync_after_gst_no_byzantine_no_adversary() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, |seed, verbose| {
        const GST: u64 = 5 * DELTA;
        const X: u64 = 2 * DELTA;
        let leaders = RoundRobin::new(N4);
        let rnd = SharedRng::new(seed);
        let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
            .rng(rnd.clone())
            .start_in_interval(&rnd, 0, GST)
            .verbose(verbose)
            .build()?;
        let mut runner = Runner::new(setup);

        runner.run(Env::new().invariant(safety()).stop_cond(time_reached(GST)))?;
        let view_at_gst = max_view(runner.state());
        let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();

        runner.run(
            Env::new()
                .invariant(safety())
                .stop_cond(move |state| max_view(state) >= epoch_view)
                .deadline(GST + 3 * VIEW_TIMEOUT),
        )?;
        let first_entered_at = runner.state().now();

        runner.run(
            Env::new()
                .invariant(safety())
                .stop_cond(move |state| min_view(state) >= epoch_view)
                .deadline(first_entered_at + X),
        )
    })
}

fn setup(n: u32, delta: u64, view_timeout: u64, lock_timeout: u64) -> Setup {
    Setup::new(n)
        .delay(delta)
        .timeouts(view_timeout, lock_timeout)
        .verbose(VERBOSE)
}

