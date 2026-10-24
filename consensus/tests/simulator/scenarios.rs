use super::dsl::*;
use super::simulator::Packet;
use anyhow::{Result, ensure};
use hotstuff_types::node::NodeID;

const DELTA: u64 = 10;
const VIEW_TIMEOUT: u64 = 100;
const LOCK_TIMEOUT: u64 = 30;

fn setup(n: u32) -> Setup {
    Setup::new(n)
        .delay(DELTA)
        .timeouts(VIEW_TIMEOUT, LOCK_TIMEOUT)
}

#[test]
fn synchronous_network_commits_without_timeouts() -> Result<()> {
    const END: u64 = 2_000;
    let mut sim = setup(4).build()?;
    let mut invariant = both(safety(), no_view_timeouts());
    run_until(&mut sim, &mut stateful(fixed(DELTA)), &mut invariant, END)?;

    let bound = (END - DELTA) / (2 * DELTA) - 1;
    let height = min_committed_height(sim.state());
    ensure!(height >= bound, "committed up to {height}, expected at least {bound}");
    Ok(())
}

#[test]
fn crashed_replica_does_not_stop_progress() -> Result<()> {
    let mut sim = setup(4).crashed([3]).build()?;
    eventually(
        &mut sim,
        &mut stateful(fixed(DELTA)),
        all_committed(20),
        &mut safety(),
        20 * VIEW_TIMEOUT * 10,
    )
}

#[test]
fn progress_resumes_after_leader_crash() -> Result<()> {
    let mut sim = setup(4).build()?;
    let mut invariant = safety();
    let mut net = stateful(fixed(DELTA));
    run_until(&mut sim, &mut net, &mut invariant, 500)?;

    sim.crash(NodeID::from(2));
    let height = min_committed_height(sim.state());
    eventually(
        &mut sim,
        &mut net,
        all_committed(height + 10),
        &mut invariant,
        500 + 50 * VIEW_TIMEOUT,
    )
}

#[test]
fn progress_after_gst() -> Result<()> {
    const GST: u64 = 2_000;
    for seed in 0..10 {
        let mut sim = setup(4).build()?;
        let mut invariant = safety();
        let mut chaos = stateful(until_gst(GST, DELTA, uniform(1, 400, seed)));
        run_until(&mut sim, &mut chaos, &mut invariant, GST)?;

        let height = min_committed_height(sim.state());
        eventually(
            &mut sim,
            &mut chaos,
            all_committed(height + 10),
            &mut invariant,
            GST + 50 * VIEW_TIMEOUT,
        )
        .map_err(|err| err.context(format!("seed {seed}")))?;
    }
    Ok(())
}

#[test]
fn partition_without_quorum_heals() -> Result<()> {
    const HEAL: u64 = 1_500;
    let mut sim = setup(4).build()?;
    let mut invariant = safety();
    let mut net = stateful(partition([&[0, 1]], HEAL, DELTA));
    run_until(&mut sim, &mut net, &mut invariant, HEAL)?;

    let height = min_committed_height(sim.state());
    eventually(
        &mut sim,
        &mut net,
        all_committed(height + 10),
        &mut invariant,
        HEAL + 50 * VIEW_TIMEOUT,
    )
}

#[test]
fn isolated_replica_catches_up() -> Result<()> {
    const HEAL: u64 = 1_500;
    let mut sim = setup(4).build()?;
    let mut invariant = safety();
    let mut net = stateful(isolate(1, HEAL, DELTA));
    run_until(&mut sim, &mut net, &mut invariant, HEAL)?;

    let others = [0, 2, 3]
        .map(|id| sim.state().committed_height(NodeID::from(id)))
        .into_iter()
        .min()
        .unwrap_or(0);
    ensure!(others > 0, "the connected majority made no progress while one replica was isolated");
    eventually(
        &mut sim,
        &mut net,
        all_committed(others + 5),
        &mut invariant,
        HEAL + 50 * VIEW_TIMEOUT,
    )
}

#[test]
fn delayed_proposal_to_two_replicas() -> Result<()> {
    const VIEW: u64 = 5;
    let mut sim = setup(4).build()?;
    let victims = [NodeID::from(1), NodeID::from(2)];
    let attack = when(
        move |_: &Ctx, p: &Packet| {
            p.kind() == Kind::Proposal && p.view() == Some(VIEW) && victims.contains(&p.to)
        },
        fixed(3 * VIEW_TIMEOUT),
        fixed(DELTA),
    );
    eventually(
        &mut sim,
        &mut stateful(attack),
        all_committed(20),
        &mut safety(),
        100 * VIEW_TIMEOUT,
    )
}

#[test]
fn random_delays_keep_safety() -> Result<()> {
    for seed in 0..20 {
        let mut sim = setup(4).build()?;
        run_until(
            &mut sim,
            &mut stateful(uniform(1, 5 * DELTA, seed)),
            &mut safety(),
            5_000,
        )
        .map_err(|err| err.context(format!("seed {seed}")))?;
        let height = min_committed_height(sim.state());
        ensure!(height >= 5, "seed {seed}: committed only up to {height}");
    }
    Ok(())
}
