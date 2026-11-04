use super::adversary::*;
use super::dsl::*;
use super::invariants::*;
use super::rng::SharedRng;
use super::runner::{Env, Runner};
use super::simulator::no_effects;
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};

const DELTA: u64 = 10;
const VIEW_TIMEOUT: u64 = 100;
const LOCK_TIMEOUT: u64 = 30;
const VERBOSE: bool = false;

const N4: u32 = 4;

#[test]
fn sync_happy_path_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_happy_path)
}

fn sync_happy_path(seed: u64, verbose: bool) -> Result<()> {
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
}

#[test]
fn sync_uneven_start_no_byzantine_no_adversary_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_uneven_start_no_byzantine_no_adversary)
}

fn sync_uneven_start_no_byzantine_no_adversary(seed: u64, verbose: bool) -> Result<()> {
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
}

#[test]
fn sync_after_gst_no_byzantine_no_adversary_test() -> Result<()> {
    let seeds = random_seeds(10_000);
    for_each_seed(&seeds, sync_after_gst_no_byzantine_no_adversary)
}

fn sync_after_gst_no_byzantine_no_adversary(seed: u64, verbose: bool) -> Result<()> {
    let leaders = RoundRobin::new(N4);
    let rnd = SharedRng::new(seed);
    let view_timeout = rnd.range(4 * DELTA, 20 * DELTA);
    let lock_timeout = rnd.range(DELTA, view_timeout);
    let gst = rnd.range(0, view_timeout);
    let setup = setup(N4, DELTA, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .start_in_interval(&rnd, 0, gst)
        .verbose(verbose)
        .build()?;

    let at_gst_msg = || format!("gst = {gst}");

    let mut runner = Runner::new(setup);
    runner.note(format!("gst = {gst}"));

    runner.run(Env::new().invariant(safety()).stop_cond(time_reached(gst)))?;
    runner.mark(gst, "GST");

    let view_at_gst = max_view(runner.state());
    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    runner.note(format!(
        "view_at_gst = {view_at_gst}, epoch_view = {epoch_view}"
    ));
    runner
        .run(
            Env::new()
                .invariant(safety())
                .stop_cond(move |state| max_view(state) >= epoch_view),
        )
        .with_context(at_gst_msg)?;

    let first_entered_at = runner.state().now();
    runner.note(format!("first_entered_at = {first_entered_at}"));
    runner
        .run(
            Env::new()
                .invariant(safety())
                .stop_cond(move |state| min_view(state) >= epoch_view)
                .deadline(first_entered_at + 2 * DELTA),
        )
        .with_context(at_gst_msg)
}

#[test]
fn sync_after_gst_tc_waits_for_isolated_node_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_after_gst_tc_waits_for_isolated_node)
}

#[test]
fn replay_seed() -> Result<()> {
    let seed = 500;
    sync_after_gst_tc_waits_for_isolated_node(seed, true)
}

fn sync_after_gst_tc_waits_for_isolated_node(seed: u64, verbose: bool) -> Result<()> {
    let leaders = RoundRobin::new(N4);
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let gst = rnd.range(7 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .byzantine([0])
        .verbose(true)
        .build()?;

    let env = || {
        let adversary = Adversary::new().delivery(isolate_node_from_all(3, gst, delta));
        Env::new().adversary(adversary).invariant(safety())
    };
    let env_n0_silent = || {
        let adversary = Adversary::new()
            .delivery(isolate_node_from_all(3, gst, delta))
            .byzantine(0, |_: &State| true, |_, _| Ok(no_effects()));
        Env::new().adversary(adversary).invariant(safety())
    };
    let at_gst_msg = || format!("gst = {gst}");

    let mut runner = Runner::new(setup);
    runner.note(format!("gst = {gst}"));

    runner.run(env().stop_cond(time_reached(gst)))?;
    runner.mark(gst, "GST");

    let view_at_gst = max_view(runner.state());
    ensure!(
        min_view(runner.state()) < view_at_gst,
        "n3 does not lag at gst = {gst}"
    );

    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    runner.note(format!(
        "view_at_gst = {view_at_gst}, epoch_view = {epoch_view}"
    ));
    runner
        .run(env_n0_silent().stop_cond(move |state| max_view(state) >= epoch_view))
        .with_context(at_gst_msg)?;

    let first_entered_at = runner.state().now();
    runner.note(format!("first_entered_at = {first_entered_at}"));
    runner
        .run(
            env_n0_silent()
                .stop_cond(move |state| min_view(state) >= epoch_view)
                .deadline(first_entered_at + 2 * delta),
        )
        .with_context(at_gst_msg)?;

    let height = min_committed_height(runner.state());
    runner.run(env_n0_silent().stop_cond(time_reached(first_entered_at + 30 * view_timeout)))?;
    let new_height = min_committed_height(runner.state());
    ensure!(
        new_height > height,
        "no commits after entering view {epoch_view}: height {height}, gst = {gst}"
    );

    Ok(())
}

fn setup(n: u32, delta: u64, view_timeout: u64, lock_timeout: u64) -> Setup {
    Setup::new(n)
        .delay(delta)
        .timeouts(view_timeout, lock_timeout)
        .verbose(VERBOSE)
}
