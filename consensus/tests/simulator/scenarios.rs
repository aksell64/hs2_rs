use super::adversary::*;
use super::byzantine::{conflicting_proposal, tail_fork};
use super::dsl::*;
use super::invariants::*;
use super::rng::SharedRng;
use super::runner::{Env, Runner};
use super::simulator::no_effects;
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};
use hotstuff_types::node::NodeID;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

const DELTA: u64 = 10;
const VIEW_TIMEOUT: u64 = 100;
const LOCK_TIMEOUT: u64 = 30;
const VERBOSE: bool = false;

const N4: u32 = 4;
const N7: u32 = 7;

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
    runner.mark(first_entered_at, &format!("first in v{epoch_view}"));
    // The first node enters on a TC or, as the leader, on a QC. A TC reaches the others within
    // DELTA. A leader missing the block of its QC must fetch it before proposing, so the others
    // get the Proposal after sync_delay + DELTA. The simulator delivers a sync as a single packet
    // under the same delay as any message, hence sync_delay = DELTA.
    let sync_delay = DELTA;
    runner
        .run(
            Env::new()
                .invariant(safety())
                .stop_cond(move |state| min_view(state) >= epoch_view)
                .deadline(first_entered_at + DELTA + sync_delay),
        )
        .with_context(at_gst_msg)?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));

    Ok(())
}

#[test]
fn sync_after_gst_tc_waits_for_isolated_node_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_after_gst_tc_waits_for_isolated_node)
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
    runner.mark(first_entered_at, &format!("first in v{epoch_view}"));
    // The first node enters on a TC or, as the leader, on a QC. A TC reaches the others within
    // delta. A leader missing the block of its QC must fetch it before proposing, so the others
    // get the Proposal after sync_delay + delta. The simulator delivers a sync as a single packet
    // under the same delay as any message, hence sync_delay = delta.
    let sync_delay = delta;
    runner
        .run(
            env_n0_silent()
                .stop_cond(move |state| min_view(state) >= epoch_view)
                .deadline(first_entered_at + delta + sync_delay),
        )
        .with_context(at_gst_msg)?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));

    // wait at most τ + 8Δ for a new commit. In the worst case n0 leads epoch_view and stays silent:
    // the view timer takes τ, n1 then waits for its lock timer (3Δ), and Proposal, votes, Proposal,
    // votes, Proposal take 5Δ.
    let height = max_committed_height(runner.state());
    runner
        .run(
            env_n0_silent()
                .stop_cond(move |state| min_committed_height(state) > height)
                .deadline(first_entered_at + view_timeout + lock_timeout + 5 * delta),
        )
        .with_context(|| {
            format!("no commits after entering view {epoch_view}: height {height}, gst = {gst}")
        })
}

#[test]
fn sync_after_gst_random_delays_no_byzantine_test() -> Result<()> {
    let seeds = random_seeds(10_000);
    for_each_seed(&seeds, sync_after_gst_random_delays_no_byzantine)
}

fn sync_after_gst_random_delays_no_byzantine(seed: u64, verbose: bool) -> Result<()> {
    const VIEWS_AFTER_ENTRY: u64 = 10;
    let leaders = RoundRobin::new(N4);
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let gst = rnd.range(view_timeout, 30 * view_timeout);
    let max_delay = rnd.range(delta, 5 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let env = || {
        let delivery = until_gst(gst, delta, uniform(0, max_delay, rnd.clone()));
        Env::new()
            .adversary(Adversary::new().delivery(delivery))
            .invariant(safety())
    };
    let at_gst_msg = || format!("gst = {gst}, max_delay = {max_delay}");

    let mut runner = Runner::new(setup);
    runner.note(at_gst_msg());

    runner.run(env().stop_cond(time_reached(gst)))?;
    runner.mark(gst, "GST");

    let view_at_gst = max_view(runner.state());
    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    runner.note(format!(
        "view_at_gst = {view_at_gst}, epoch_view = {epoch_view}"
    ));
    runner
        .run(env().stop_cond(move |state| max_view(state) >= epoch_view))
        .with_context(at_gst_msg)?;

    let first_entered_at = runner.state().now();
    runner.note(format!("first_entered_at = {first_entered_at}"));
    runner.mark(first_entered_at, &format!("first in v{epoch_view}"));
    // The first node enters on a TC or, as the leader, on a QC. A TC reaches the others within
    // delta. A leader missing the block of its QC must fetch it before proposing, so the others
    // get the Proposal after sync_delay + delta. The simulator delivers a sync as a single packet
    // under the same delay as any message, hence sync_delay = delta.
    let sync_delay = delta;
    runner
        .run(
            env()
                .stop_cond(move |state| min_view(state) >= epoch_view)
                .deadline(first_entered_at + delta + sync_delay),
        )
        .with_context(at_gst_msg)?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));

    // The leader of epoch_view may wait for its lock timer (3Δ). Then Proposal, votes and the next
    // Proposal take 3Δ, and blocks delayed before GST can cost two fetches: by the leader before it
    // proposes and by the voters before they vote.
    let entered_at = runner.state().now();
    let timeouts = runner.state().view_timeouts();
    let next_view = epoch_view + 1;
    runner
        .run(
            env()
                .stop_cond(move |state| min_view(state) >= next_view)
                .deadline(entered_at + lock_timeout + 3 * delta + 2 * sync_delay),
        )
        .with_context(at_gst_msg)?;

    // From here on votes and the next Proposal take 2Δ, and entering a view commits the block of
    // two views before, so only the blocks of the last two views stay uncommitted.
    for view in epoch_view + 2..=epoch_view + VIEWS_AFTER_ENTRY {
        let entered_at = runner.state().now();
        let height = min_committed_height(runner.state());
        runner
            .run(
                env()
                    .stop_cond(move |state| min_view(state) >= view)
                    .deadline(entered_at + 2 * delta),
            )
            .with_context(at_gst_msg)?;
        let new_height = min_committed_height(runner.state());
        ensure!(
            new_height > height && new_height == max_committed_height(runner.state()),
            "no commit on entering view {view}: height {height} -> {new_height}, {}",
            at_gst_msg()
        );
    }
    ensure!(
        runner.state().view_timeouts() == timeouts,
        "view timers fired after entering view {epoch_view}, {}",
        at_gst_msg()
    );

    Ok(())
}

#[test]
fn tail_fork_drops_blocks_of_previous_leader_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, tail_fork_drops_blocks_of_previous_leader)
}

// n3 holds QC(v - 1) but builds on QC(v - 2), so the blocks of n2 are never committed. This pins a
// known weakness of the protocol: a defense against tail-forking is expected to break this test.
fn tail_fork_drops_blocks_of_previous_leader(seed: u64, verbose: bool) -> Result<()> {
    const CYCLES: u64 = 10;
    const FIRST_VIEW_OF_N2: u64 = 2;
    const LAST_VIEW: u64 = FIRST_VIEW_OF_N2 + CYCLES * N4 as u64;
    let rnd = SharedRng::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .byzantine([3])
        .verbose(verbose)
        .build()?;

    let adversary = Adversary::new().byzantine(3, |_: &State| true, tail_fork(N4));
    let mut runner = Runner::new(setup);
    // The first 3Δ are Proposal, votes and Proposal up to the first view of n2. Then in each cycle
    // others do not enter the view of n3 on its Proposal, because the QC in it is too old, so they
    // wait for the view timer (τ). By then n0 has the votes of n3 and of n2, whose timer fired Δ
    // earlier, so its own vote completes the QC at once. Then Proposal, votes, Proposal, votes and
    // Proposal take 5Δ.
    runner.run(
        Env::new()
            .adversary(adversary)
            .invariant(safety())
            .stop_cond(|state| min_view(state) >= LAST_VIEW)
            .deadline(3 * delta + CYCLES * (view_timeout + 5 * delta)),
    )?;

    // Every view has one block. One block of n2 is dropped per cycle, and the blocks of the last
    // two views are not committed yet.
    let height = min_committed_height(runner.state());
    let expected_height = LAST_VIEW - CYCLES - 2;
    ensure!(
        height == expected_height && height == max_committed_height(runner.state()),
        "expected height {expected_height} on every node, min is {height}"
    );

    let expected = BTreeSet::from([0u32, 1, 3].map(NodeID::from));
    for id in runner.state().honest_nodes() {
        let proposers = committed_proposers(runner.state(), id)?;
        ensure!(
            proposers == expected,
            "{id:?} committed blocks of {proposers:?}, expected {expected:?}"
        );
    }

    Ok(())
}

#[test]
fn byzantine_leader_conflicting_proposal_is_rejected_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, byzantine_leader_conflicting_proposal_is_rejected)
}

// G -> B(v1) -> C(v2). After collecting QC(C), n3 withholds it and proposes G -> B'
// in v3. B' has a genuine QC(G), but honest voters for C are already locked on B.
fn byzantine_leader_conflicting_proposal_is_rejected(seed: u64, verbose: bool) -> Result<()> {
    const ATTACK_VIEW: u64 = 3;
    let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
        .rng(SharedRng::new(seed))
        .byzantine([3])
        .verbose(verbose)
        .build()?;
    let attack = Rc::new(Cell::new(None));
    let recorded_attack = Rc::clone(&attack);
    let adversary = Adversary::new().byzantine(
        3,
        |_: &State| true,
        conflicting_proposal(N4, ATTACK_VIEW, move |protected, fork| {
            recorded_attack.set(Some((protected, fork)));
        }),
    );

    let mut runner = Runner::new(setup);
    runner.run(
        Env::new()
            .adversary(adversary)
            .invariant(safety())
            // The stale QC cannot advance peers to v3. Wait for their timers and the replay
            // of the buffered fork, so rejection cannot be explained by a wrong current view.
            .stop_cond(|state| min_view(state) >= ATTACK_VIEW)
            .deadline(2 * VIEW_TIMEOUT),
    )?;
    let (protected, fork) = attack
        .get()
        .context("n3 never proposed the conflicting block")?;
    proposal_rejected(ATTACK_VIEW, fork, 2, (1, protected))(runner.state())?;
    runner.mark(runner.state().now(), "conflicting proposal rejected");

    let silent = Adversary::new().byzantine(3, |_: &State| true, |_, _| Ok(no_effects()));
    runner.run(
        Env::new()
            .adversary(silent)
            .invariant(safety())
            .stop_cond(|state| min_committed_height(state) >= 2)
            .deadline(5 * VIEW_TIMEOUT),
    )?;
    for (id, log) in runner.state().honest_commit_logs() {
        ensure!(
            log.iter()
                .any(|commit| commit.height == 1 && commit.hash == protected),
            "{id:?} did not commit the protected block B"
        );
        ensure!(
            log.iter().all(|commit| commit.hash != fork),
            "{id:?} committed the conflicting block"
        );
    }
    Ok(())
}

#[test]
fn sync_after_gst_tc_waits_for_lagging_nodes_test() -> Result<()> {
    sync_after_gst_tc_waits_for_lagging_nodes(0, true)
}

fn sync_after_gst_tc_waits_for_lagging_nodes(seed: u64, verbose: bool) -> Result<()> {
    let delta = 5;
    let view_timeout = 20 * delta;
    let lock_timeout = 3 * delta;
    let gst = 10 * view_timeout;
    let end = gst + 10 * view_timeout;
    let setup = setup(N7, delta, view_timeout, lock_timeout)
        .rng(SharedRng::new(seed))
        .byzantine([0, 1])
        .verbose(verbose)
        .build()?;

    let lagging = || partition([&[5, 6]], gst, delta);
    let mut runner = Runner::new(setup);
    runner.run(
        Env::new()
            .adversary(Adversary::new().delivery(lagging()))
            .invariant(safety())
            .stop_cond(time_reached(gst)),
    )?;
    runner.mark(gst, "GST");

    let leaders = RoundRobin::new(N7);
    let view_at_gst = max_view(runner.state());
    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    let silent = || {
        let adversary = Adversary::new()
            .delivery(lagging())
            .byzantine(0, |_: &State| true, |_, _| Ok(no_effects()))
            .byzantine(1, |_: &State| true, |_, _| Ok(no_effects()));
        Env::new().adversary(adversary).invariant(safety())
    };
    runner.run(silent().stop_cond(move |state| max_view(state) >= epoch_view))?;
    runner.mark(runner.state().now(), &format!("first in v{epoch_view}"));
    runner.run(silent().stop_cond(move |state| min_view(state) >= epoch_view))?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));
    runner.run(silent().stop_cond(time_reached(end)))
}

#[test]
fn replay_seed() -> Result<()> {
    let seed = 500;
    byzantine_leader_conflicting_proposal_is_rejected(500, true)
}

fn setup(n: u32, delta: u64, view_timeout: u64, lock_timeout: u64) -> Setup {
    Setup::new(n)
        .delay(delta)
        .timeouts(view_timeout, lock_timeout)
        .verbose(VERBOSE)
}
