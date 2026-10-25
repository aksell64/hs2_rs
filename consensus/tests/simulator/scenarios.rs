use super::adversary::*;
use super::dsl::*;
use super::invariants::*;
use super::runner::{Env, Runner};
use anyhow::{Result, ensure};
use hotstuff_types::node::NodeID;

const DELTA: u64 = 10;
const VIEW_TIMEOUT: u64 = 100;
const LOCK_TIMEOUT: u64 = 30;
const VERBOSE: bool = true;

#[test]
fn synchronous_network_commits_without_timeouts() -> Result<()> {
    const END: u64 = 2_000;
    let mut runner = Runner::new(setup(4).build()?);
    runner.run(
        Env::new()
            .invariant(safety())
            .invariant(no_view_timeouts())
            .stop_cond(time_reached(END)),
    )?;

    let bound = (END - DELTA) / (2 * DELTA) - 1;
    let height = min_committed_height(runner.state());
    ensure!(
        height >= bound,
        "committed up to {height}, expected at least {bound}"
    );
    Ok(())
}

#[test]
fn synchronous_network_after_uneven_entrance() -> Result<()> {
    const END: u64 = 2_000;
    const LAST_START: u64 = 40;
    let mut runner = Runner::new(
        setup(4)
            .start_at([(0, 10), (1, 20), (2, 30), (3, LAST_START)])
            .build()?,
    );
    runner.run(Env::new().invariant(safety()).stop_cond(time_reached(END)))?;

    let bound = (END - LAST_START - DELTA) / (2 * DELTA) - 1 - VIEW_TIMEOUT / (2 * DELTA);
    let height = min_committed_height(runner.state());
    ensure!(
        height >= bound,
        "committed up to {height}, expected at least {bound}"
    );
    Ok(())
}

#[test]
fn synchronous_bound_holds_for_any_tie_order() -> Result<()> {
    const END: u64 = 2_000;
    let bound = (END - DELTA) / (2 * DELTA) - 1;
    for seed in 0..20 {
        let mut runner = Runner::new(setup(4).shuffle(seed).build()?);
        runner
            .run(
                Env::new()
                    .invariant(safety())
                    .invariant(no_view_timeouts())
                    .stop_cond(time_reached(END)),
            )
            .map_err(|err| err.context(format!("seed {seed}")))?;
        let height = min_committed_height(runner.state());
        ensure!(
            height >= bound,
            "seed {seed}: committed up to {height}, expected at least {bound}"
        );
    }
    Ok(())
}

#[test]
fn crashed_replica_does_not_stop_progress() -> Result<()> {
    Runner::new(setup(4).crashed([3]).build()?).run(
        Env::new()
            .invariant(safety())
            .stop_cond(all_committed(20))
            .deadline(20 * VIEW_TIMEOUT * 10),
    )?;
    Ok(())
}

#[test]
fn progress_resumes_after_leader_crash() -> Result<()> {
    let mut runner = Runner::new(setup(4).build()?);
    runner.run(Env::new().invariant(safety()).stop_cond(time_reached(500)))?;
    runner.crash(NodeID::from(2));

    let height = min_committed_height(runner.state());
    runner.run(
        Env::new()
            .invariant(safety())
            .stop_cond(all_committed(height + 10))
            .deadline(500 + 50 * VIEW_TIMEOUT),
    )?;
    Ok(())
}

#[test]
fn progress_after_gst() -> Result<()> {
    const GST: u64 = 2_000;
    for seed in 0..10 {
        let mut runner = Runner::new(setup(4).build()?);
        let delivery = until_gst(GST, DELTA, uniform(1, 400, seed));
        runner
            .run(
                Env::new()
                    .adversary(Adversary::new().delivery(delivery))
                    .invariant(safety())
                    .stop_cond(time_reached(GST)),
            )
            .map_err(|err| err.context(format!("seed {seed}")))?;

        let height = min_committed_height(runner.state());
        runner
            .run(
                Env::new()
                    .invariant(safety())
                    .stop_cond(all_committed(height + 10))
                    .deadline(GST + 50 * VIEW_TIMEOUT),
            )
            .map_err(|err| err.context(format!("seed {seed}")))?;
    }
    Ok(())
}

#[test]
fn partition_without_quorum_heals() -> Result<()> {
    const HEAL: u64 = 1_500;
    let mut runner = Runner::new(setup(4).build()?);
    runner.run(
        Env::new()
            .adversary(Adversary::new().delivery(partition([&[0, 1]], HEAL, DELTA)))
            .invariant(safety())
            .stop_cond(time_reached(HEAL)),
    )?;

    let height = min_committed_height(runner.state());
    runner.run(
        Env::new()
            .invariant(safety())
            .stop_cond(all_committed(height + 10))
            .deadline(HEAL + 50 * VIEW_TIMEOUT),
    )?;
    Ok(())
}

#[test]
fn isolated_replica_catches_up() -> Result<()> {
    const HEAL: u64 = 1_500;
    let mut runner = Runner::new(setup(4).build()?);
    runner.run(
        Env::new()
            .adversary(Adversary::new().delivery(isolate(1, HEAL, DELTA)))
            .invariant(safety())
            .stop_cond(time_reached(HEAL)),
    )?;

    let others = [0, 2, 3]
        .map(|id| runner.state().committed_height(NodeID::from(id)))
        .into_iter()
        .min()
        .unwrap_or(0);
    ensure!(
        others > 0,
        "the connected majority made no progress while one replica was isolated"
    );
    runner.run(
        Env::new()
            .invariant(safety())
            .stop_cond(all_committed(others + 5))
            .deadline(HEAL + 50 * VIEW_TIMEOUT),
    )?;
    Ok(())
}

#[test]
fn delayed_proposal_to_two_replicas() -> Result<()> {
    const VIEW: u64 = 5;
    let victims = [NodeID::from(1), NodeID::from(2)];
    let attack = when(
        move |_: &Ctx, p: &Packet| {
            p.kind() == Kind::Proposal && p.view() == Some(VIEW) && victims.contains(&p.to)
        },
        fixed(3 * VIEW_TIMEOUT),
        fixed(DELTA),
    );
    Runner::new(setup(4).build()?).run(
        Env::new()
            .adversary(Adversary::new().delivery(attack))
            .invariant(safety())
            .stop_cond(all_committed(20))
            .deadline(100 * VIEW_TIMEOUT),
    )?;
    Ok(())
}

#[test]
fn random_delays_keep_safety() -> Result<()> {
    for seed in 0..20 {
        let mut runner = Runner::new(setup(4).build()?);
        runner
            .run(
                Env::new()
                    .adversary(Adversary::new().delivery(uniform(1, 5 * DELTA, seed)))
                    .invariant(safety())
                    .stop_cond(time_reached(5_000)),
            )
            .map_err(|err| err.context(format!("seed {seed}")))?;
        let height = min_committed_height(runner.state());
        ensure!(height >= 5, "seed {seed}: committed only up to {height}");
    }
    Ok(())
}

fn setup(n: u32) -> Setup {
    Setup::new(n)
        .delay(DELTA)
        .timeouts(VIEW_TIMEOUT, LOCK_TIMEOUT)
        .verbose(VERBOSE)
}

fn min_committed_height(state: &State) -> u64 {
    state
        .running()
        .map(|id| state.committed_height(id))
        .min()
        .unwrap_or(0)
}

fn time_reached(time: u64) -> impl Fn(&State) -> bool {
    move |state| state.next_time().is_none_or(|at| at > time)
}

fn all_committed(height: u64) -> impl Fn(&State) -> bool {
    move |state| min_committed_height(state) >= height
}
