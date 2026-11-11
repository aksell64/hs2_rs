use super::adversary::*;
use super::dsl::*;
use super::invariants::*;
use super::rng::SharedRng;
use super::runner::{Env, Runner};
use anyhow::{Context as _, Result, ensure};
use hotstuff_types::node::NodeID;

const N4: u32 = 4;
const LAGGING: u32 = 3;

// n3 is cut off until `heal`; the other three are a quorum and keep committing. Messages held by
// the partition arrive at heal + delta: n3 gets old Proposals whose parents it lacks, fetches the
// missing branches and replays the Proposals. That commits all but the last blocks: the block of
// the newest QC comes in no Proposal yet, and the driver fetches the block of a high QC only for a
// leader (`ProposalDeferred`), not for a voter. So n3 reaches the height the others had at `heal`
// with the next Proposal, which may come after a view timeout.
#[test]
fn sync_isolated_node_catches_up_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_isolated_node_catches_up)
}

fn sync_isolated_node_catches_up(seed: u64, verbose: bool) -> Result<()> {
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let heal = rnd.range(10 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let env = || {
        let adversary = Adversary::new().delivery(isolate_node_from_all(LAGGING, heal, delta));
        Env::new().adversary(adversary).invariant(safety())
    };
    let at_heal_msg = || format!("heal = {heal}");

    let mut runner = Runner::new(setup);
    runner.note(at_heal_msg());

    runner.run(env().stop_cond(time_reached(heal)))?;
    runner.mark(heal, "heal");

    let lagging = NodeID::from(LAGGING);
    let height = max_committed_height(runner.state());
    ensure!(
        runner.state().committed_height(lagging) < height,
        "n3 does not lag at heal = {heal}"
    );
    runner.note(format!("height at heal = {height}"));

    // Held messages: delta. Next Proposal: at most τ + lock timer + 5Δ, as for a silent leader in
    // scenarios.rs. Fetch of its parent: sync_delay = delta.
    let sync_delay = delta;
    runner
        .run(
            env()
                .stop_cond(move |state| state.committed_height(lagging) >= height)
                .deadline(heal + delta + view_timeout + lock_timeout + 5 * delta + sync_delay),
        )
        .with_context(|| format!("n3 did not catch up to height {height}, {}", at_heal_msg()))?;
    runner.mark(runner.state().now(), "n3 caught up");

    Ok(())
}

// As above, but every sync of the simulator is lost with probability one half. The driver has no
// retry of its own: a lost fetch is repeated only when the next Proposal again lacks its parent.
// So n3 catches up within a few views instead of at once.
#[test]
fn sync_lossy_fetches_catch_up_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_lossy_fetches_catch_up)
}

fn sync_lossy_fetches_catch_up(seed: u64, verbose: bool) -> Result<()> {
    const VIEWS: u64 = 20;
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let heal = rnd.range(10 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let env = || {
        let delivery = when(
            |_: &Ctx, packet: &Packet| packet.kind() == Kind::Sync,
            lossy(50, rnd.clone(), fixed(delta)),
            isolate_node_from_all(LAGGING, heal, delta),
        );
        Env::new()
            .adversary(Adversary::new().delivery(delivery))
            .invariant(safety())
    };
    let at_heal_msg = || format!("heal = {heal}");

    let mut runner = Runner::new(setup);
    runner.note(at_heal_msg());

    runner.run(env().stop_cond(time_reached(heal)))?;
    runner.mark(heal, "heal");

    let lagging = NodeID::from(LAGGING);
    let height = max_committed_height(runner.state());
    ensure!(
        runner.state().committed_height(lagging) < height,
        "n3 does not lag at heal = {heal}"
    );

    // Each view gives n3 a new fetch with its Proposal, and a view takes at most τ + lock timer +
    // 5Δ. Losing VIEWS fetches in a row has probability 2^-VIEWS.
    runner
        .run(
            env()
                .stop_cond(move |state| state.committed_height(lagging) >= height)
                .deadline(heal + delta + VIEWS * (view_timeout + lock_timeout + 5 * delta)),
        )
        .with_context(|| format!("n3 did not catch up to height {height}, {}", at_heal_msg()))?;
    runner.mark(runner.state().now(), "n3 caught up");

    Ok(())
}

// Every sync to n3 is lost. n3 never catches up, and that must not stop the others: they are a
// quorum without n3. But n3 acts as a silent node: it cannot propose in its views, and the votes
// of the view before go to it. Every cycle of four views then has two view timeouts and one commit:
// two waits of τ + lock timer + 3Δ.
#[test]
fn sync_lost_for_one_node_others_keep_committing_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_lost_for_one_node_others_keep_committing)
}

fn sync_lost_for_one_node_others_keep_committing(seed: u64, verbose: bool) -> Result<()> {
    const VIEWS: u64 = 10;
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let heal = rnd.range(10 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let env = || {
        let delivery = when(
            |_: &Ctx, packet: &Packet| {
                packet.kind() == Kind::Sync && packet.to == NodeID::from(LAGGING)
            },
            lossy(100, rnd.clone(), fixed(delta)),
            isolate_node_from_all(LAGGING, heal, delta),
        );
        Env::new()
            .adversary(Adversary::new().delivery(delivery))
            .invariant(safety())
    };
    let at_heal_msg = || format!("heal = {heal}");

    let mut runner = Runner::new(setup);
    runner.note(at_heal_msg());
    runner.run(env().stop_cond(time_reached(heal)))?;
    runner.mark(heal, "heal");

    let lagging = NodeID::from(LAGGING);
    let lagging_height = runner.state().committed_height(lagging);
    for _ in 0..VIEWS {
        let height = others_min_height(runner.state(), lagging);
        let deadline = runner.state().now() + 2 * (view_timeout + lock_timeout + 3 * delta);
        runner
            .run(
                env()
                    .stop_cond(move |state| others_min_height(state, lagging) > height)
                    .deadline(deadline),
            )
            .with_context(|| format!("others stopped committing at {height}, {}", at_heal_msg()))?;
    }
    ensure!(
        runner.state().committed_height(lagging) == lagging_height,
        "n3 committed without any sync: {lagging_height} -> {}",
        runner.state().committed_height(lagging)
    );

    Ok(())
}

// Design (docs/research/blocksync/design.md, T2): any node that learns a QC whose block it lacks
// fetches that block. The driver does so only for a leader (`ProposalDeferred`), so a voter that
// got the newest QC from a Vote, a Status or a TC waits for the next Proposal. With T2 for every
// node, n3 catches up right after the held messages: delta for them, sync_delay for the fetch.
#[test]
#[ignore = "T2 is implemented for the leader only"]
fn sync_voter_fetches_high_qc_block_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_voter_fetches_high_qc_block)
}

fn sync_voter_fetches_high_qc_block(seed: u64, verbose: bool) -> Result<()> {
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let heal = rnd.range(10 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let env = || {
        let adversary = Adversary::new().delivery(isolate_node_from_all(LAGGING, heal, delta));
        Env::new().adversary(adversary).invariant(safety())
    };
    let at_heal_msg = || format!("heal = {heal}");

    let mut runner = Runner::new(setup);
    runner.note(at_heal_msg());
    runner.run(env().stop_cond(time_reached(heal)))?;
    runner.mark(heal, "heal");

    let lagging = NodeID::from(LAGGING);
    let height = max_committed_height(runner.state());
    let sync_delay = delta;
    runner
        .run(
            env()
                .stop_cond(move |state| state.committed_height(lagging) >= height)
                .deadline(heal + delta + 2 * sync_delay),
        )
        .with_context(|| format!("n3 did not catch up to height {height}, {}", at_heal_msg()))?;

    Ok(())
}

// Design (4.3): a fetch that gets no response times out and goes to the next peer. The driver
// has no retry: a lost fetch is repeated only when another Proposal lacks its parent. Here only
// the first sync of n3 after heal is lost; with a retry n3 catches up one sync_delay later than in
// `sync_voter_fetches_high_qc_block`, plus the retry timeout, taken here as delta. The bound
// assumes T2 for voters too, so this test also needs that fix.
#[test]
#[ignore = "the driver does not retry a lost fetch"]
fn sync_lost_fetch_is_retried_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_lost_fetch_is_retried)
}

fn sync_lost_fetch_is_retried(seed: u64, verbose: bool) -> Result<()> {
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let heal = rnd.range(10 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let env = || {
        let mut dropped = false;
        let delivery = when(
            move |context: &Ctx, packet: &Packet| {
                let first = !dropped
                    && context.now >= heal
                    && packet.kind() == Kind::Sync
                    && packet.to == NodeID::from(LAGGING);
                dropped |= first;
                first
            },
            lossy(100, rnd.clone(), fixed(delta)),
            isolate_node_from_all(LAGGING, heal, delta),
        );
        Env::new()
            .adversary(Adversary::new().delivery(delivery))
            .invariant(safety())
    };
    let at_heal_msg = || format!("heal = {heal}");

    let mut runner = Runner::new(setup);
    runner.note(at_heal_msg());
    runner.run(env().stop_cond(time_reached(heal)))?;
    runner.mark(heal, "heal");

    let lagging = NodeID::from(LAGGING);
    let height = max_committed_height(runner.state());
    let sync_delay = delta;
    let retry_timeout = delta;
    runner
        .run(
            env()
                .stop_cond(move |state| state.committed_height(lagging) >= height)
                .deadline(heal + delta + retry_timeout + 3 * sync_delay),
        )
        .with_context(|| format!("n3 did not catch up to height {height}, {}", at_heal_msg()))?;

    Ok(())
}

fn others_min_height(state: &State, lagging: NodeID) -> u64 {
    state
        .honest_nodes()
        .filter(|id| *id != lagging)
        .map(|id| state.committed_height(id))
        .min()
        .unwrap_or(0)
}

fn setup(n: u32, delta: u64, view_timeout: u64, lock_timeout: u64) -> Setup {
    Setup::new(n)
        .delay(delta)
        .timeouts(view_timeout, lock_timeout)
}
