use super::adversary::*;
use super::dsl::*;
use super::invariants::*;
use super::runner::Runner;
use anyhow::{Result, bail, ensure};
use hotstuff_types::node::NodeID;

const DELTA: u64 = 10;
const VIEW_TIMEOUT: u64 = 100;
const LOCK_TIMEOUT: u64 = 30;
const VERBOSE: bool = true;

fn setup(n: u32) -> Setup {
    Setup::new(n)
        .delay(DELTA)
        .timeouts(VIEW_TIMEOUT, LOCK_TIMEOUT)
        .verbose(VERBOSE)
}

#[test]
fn synchronous_network_commits_without_timeouts() -> Result<()> {
    const END: u64 = 2_000;
    let runner = Runner::new(setup(4).build()?)
        .invariant(safety())
        .invariant(no_view_timeouts())
        .stop_cond(time_reached(END))
        .run()?;

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
    let runner = Runner::new(
        setup(4)
            .start_at([(0, 10), (1, 20), (2, 30), (3, LAST_START)])
            .build()?,
    )
        .invariant(safety())
        .stop_cond(time_reached(END))
        .run()?;

    let bound = (END - LAST_START - DELTA) / (2 * DELTA) - 1 - VIEW_TIMEOUT / (2 * DELTA);
    let height = min_committed_height(runner.state());
    ensure!(height >= bound, "committed up to {height}, expected at least {bound}");
    Ok(())
}

#[test]
fn synchronous_bound_holds_for_any_tie_order() -> Result<()> {
    const END: u64 = 2_000;
    let bound = (END - DELTA) / (2 * DELTA) - 1;
    for seed in 0..20 {
        let runner = Runner::new(setup(4).shuffle(seed).build()?)
            .invariant(safety())
            .invariant(no_view_timeouts())
            .stop_cond(time_reached(END))
            .run()
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
    Runner::new(setup(4).crashed([3]).build()?)
        .invariant(safety())
        .stop_cond(all_committed(20))
        .deadline(20 * VIEW_TIMEOUT * 10)
        .run()?;
    Ok(())
}

#[test]
fn progress_resumes_after_leader_crash() -> Result<()> {
    let runner = Runner::new(setup(4).build()?)
        .invariant(safety())
        .stop_cond(time_reached(500))
        .run()?
        .crash(NodeID::from(2));

    let height = min_committed_height(runner.state());
    runner
        .stop_cond(all_committed(height + 10))
        .deadline(500 + 50 * VIEW_TIMEOUT)
        .run()?;
    Ok(())
}

#[test]
fn progress_after_gst() -> Result<()> {
    const GST: u64 = 2_000;
    for seed in 0..10 {
        Runner::new(setup(4).build()?)
            .adversary(Adversary::new().delivery(until_gst(GST, DELTA, uniform(1, 400, seed))))
            .invariant(safety())
            .progress_after(GST, 10, 50 * VIEW_TIMEOUT)
            .map_err(|err| err.context(format!("seed {seed}")))?;
    }
    Ok(())
}

#[test]
fn partition_without_quorum_heals() -> Result<()> {
    const HEAL: u64 = 1_500;
    Runner::new(setup(4).build()?)
        .adversary(Adversary::new().delivery(partition([&[0, 1]], HEAL, DELTA)))
        .invariant(safety())
        .progress_after(HEAL, 10, 50 * VIEW_TIMEOUT)?;
    Ok(())
}

#[test]
fn isolated_replica_catches_up() -> Result<()> {
    const HEAL: u64 = 1_500;
    let runner = Runner::new(setup(4).build()?)
        .adversary(Adversary::new().delivery(isolate(1, HEAL, DELTA)))
        .invariant(safety())
        .stop_cond(time_reached(HEAL))
        .run()?;

    let others = [0, 2, 3]
        .map(|id| runner.state().committed_height(NodeID::from(id)))
        .into_iter()
        .min()
        .unwrap_or(0);
    ensure!(
        others > 0,
        "the connected majority made no progress while one replica was isolated"
    );
    runner
        .stop_cond(all_committed(others + 5))
        .deadline(HEAL + 50 * VIEW_TIMEOUT)
        .run()?;
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
    Runner::new(setup(4).build()?)
        .adversary(Adversary::new().delivery(attack))
        .invariant(safety())
        .stop_cond(all_committed(20))
        .deadline(100 * VIEW_TIMEOUT)
        .run()?;
    Ok(())
}

#[test]
fn random_delays_keep_safety() -> Result<()> {
    for seed in 0..20 {
        let runner = Runner::new(setup(4).build()?)
            .adversary(Adversary::new().delivery(uniform(1, 5 * DELTA, seed)))
            .invariant(safety())
            .stop_cond(time_reached(5_000))
            .run()
            .map_err(|err| err.context(format!("seed {seed}")))?;
        let height = min_committed_height(runner.state());
        ensure!(height >= 5, "seed {seed}: committed only up to {height}");
    }
    Ok(())
}
