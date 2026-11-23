use super::adversary::*;
use super::byzantine::{
    ByzantineProxy, conflicting_proposal, equivocation as equivocation_behavior, reveal_qc_to_one,
    tail_fork, unlimited_votes_and_qcs,
};
use super::dsl::*;
use super::invariants::*;
use super::rng::Randomness;
use super::runner::{Env, Runner};
use super::simulator::{Context, no_effects};
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
fn replay_seed() -> Result<()> {
    let seed = 500;
    replay(seed, equivocation_with_unlimited_votes)
}

#[test]
fn equivocation_test() -> Result<()> {
    for_each_seed(&random_seeds(1_0_000), equivocation)
}

fn equivocation(seed: u64, verbose: bool) -> Result<()> {
    let rnd = Randomness::new(seed);
    let n = rnd.range(4, 10) as u32;
    let f = rnd.range(1, u64::from((n - 1) / 3)) as u32;
    let delta = rnd.range(5, 30);
    let gst = rnd.range(1, 50 * delta);
    let lock_timeout = rnd.range(3 * delta + 1, 5 * delta);
    let view_timeout = rnd.range(lock_timeout + 7 * delta + 1, lock_timeout + 10 * delta);

    let setup = setup(n, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .start_in_interval(&rnd, 1, delta * 10)
        .build()?;

    let delivery = move |_: &Ctx, packet: &Packet| match packet.kind() {
        Kind::Sync => Fate::Delay(0),
        _ => Fate::Delay(delta),
    };
    let adversary = Adversary::new(rnd.sample_exact(n, f))
        .delivery(delivery)
        .byzantine_node(NodeSelector::Range(f), || {
            ByzantineProxy::new(
                "eviction",
                with_probability::<VirtualClock>(70),
                equivocation_behavior(),
            )
        });

    let mut runner = Runner::new(setup);

    let mut env = Env::new()
        .adversary(adversary)
        .stop_cond(time_reached(gst))
        .invariant(safety());
    runner.run(&mut env)?;
    runner.mark(gst, "GST");

    if runner.context(&env).honest_unstarted().count() > 0 {
        env = env
            .stop_cond(|ctx| ctx.honest_unstarted().count() == 0)
            .deadline(gst.max(10 * delta));
        runner.run(&mut env)?;
    }

    let ctx = runner.context(&env);
    let leaders = RoundRobin::new(n);
    let epoch_view = leaders.epoch_end(max_view(&ctx)) + 1;

    env = env.stop_cond(move |ctx| max_view(ctx) >= epoch_view);
    runner.run(&mut env)?;

    let epoch_entered_at = runner.state().now();
    let height_before = max_committed_height(&runner.context(&env));
    runner.note(format!(
        "gst = {gst}, epoch_view = {epoch_view}, height_before = {height_before}"
    ));
    // L6–L7: (n - 1)τ + LT + 9δ < nτ + 2δ, even across the epoch boundary.
    let commit_deadline = epoch_entered_at + leaders.epoch_length() * view_timeout + 2 * delta;
    runner.mark(
        epoch_entered_at,
        &format!("first honest in epoch {epoch_view}"),
    );
    runner.note(format!(
        "commit deadline = {commit_deadline}, network delay = {delta}, sync delay = 0"
    ));
    env = env
        .stop_cond(move |ctx| min_committed_height(ctx) > height_before)
        .deadline(commit_deadline);
    runner.run(&mut env).with_context(|| {
        format!(
            "no progress from epoch {epoch_view} by {commit_deadline}: \
             max height before = {height_before}, epoch entered at = {epoch_entered_at}, seed = {seed}"
        )
    })?;
    Ok(())
}

#[test]
fn equivocation_with_unlimited_votes_test() -> Result<()> {
    for_each_seed(&random_seeds(1_0_000), equivocation_with_unlimited_votes)
}

fn equivocation_with_unlimited_votes(seed: u64, verbose: bool) -> Result<()> {
    let rnd = Randomness::new(seed);
    let n = rnd.range(4, 10) as u32;
    let f = rnd.range(1, u64::from((n - 1) / 3)) as u32;
    let delta = rnd.range(5, 30);
    let gst = rnd.range(1, 50 * delta);
    let lock_timeout = rnd.range(3 * delta + 1, 5 * delta);
    let view_timeout = rnd.range(lock_timeout + 7 * delta + 1, lock_timeout + 10 * delta);

    let setup = setup(n, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .start_in_interval(&rnd, 1, delta * 10)
        .build()?;

    let delivery = move |_: &Ctx, packet: &Packet| match packet.kind() {
        Kind::Sync => Fate::Delay(0),
        _ => Fate::Delay(delta),
    };
    let mut adversary = Adversary::new(rnd.sample_exact(n, f))
        .delivery(delivery)
        .byzantine_node(NodeSelector::All, || {
            ByzantineProxy::new(
                "eviction",
                with_probability::<VirtualClock>(70),
                equivocation_behavior(),
            )
        });

    let honest: Vec<_> = (0..n)
        .map(NodeID::from)
        .filter(|id| !adversary.byzantine_nodes.contains(id))
        .collect();
    // let mutant_count = rnd.range(1, u64::from(n - f)) as u32;
    let mutant_count = n-f;
    let mutants: Vec<_> = rnd
        .sample_exact(n - f, mutant_count)
        .into_iter()
        .map(|index| honest[index as usize])
        .collect();
    for &id in &mutants {
        adversary = adversary.mutate_node(NodeSelector::Id(id), || {
            ByzantineProxy::new(
                "unlimited_votes_and_qcs",
                |_: &Context<'_, VirtualClock>| true,
                unlimited_votes_and_qcs(),
            )
        });
    }

    let mut runner = Runner::new(setup);
    runner.note(format!(
        "unlimited votes and multiple QCs: {mutant_count} mutants, nodes = {mutants:?}"
    ));

    let mut env = Env::new()
        .adversary(adversary)
        .stop_cond(time_reached(gst))
        .invariant(safety());
    runner.run(&mut env)?;
    runner.mark(gst, "GST");

    if runner.context(&env).honest_unstarted().count() > 0 {
        env = env
            .stop_cond(|ctx| ctx.honest_unstarted().count() == 0)
            .deadline(gst.max(10 * delta));
        runner.run(&mut env)?;
    }

    let ctx = runner.context(&env);
    let leaders = RoundRobin::new(n);
    let epoch_view = leaders.epoch_end(max_view(&ctx)) + 1;

    env = env.stop_cond(move |ctx| max_view(ctx) >= epoch_view);
    runner.run(&mut env)?;

    let epoch_entered_at = runner.state().now();
    let height_before = max_committed_height(&runner.context(&env));
    runner.note(format!(
        "gst = {gst}, epoch_view = {epoch_view}, height_before = {height_before}"
    ));
    // Probe the original eviction progress bound; mutations can invalidate it.
    let commit_deadline = epoch_entered_at + leaders.epoch_length() * view_timeout + 2 * delta;
    runner.mark(
        epoch_entered_at,
        &format!("first honest in epoch {epoch_view}"),
    );
    runner.note(format!(
        "commit deadline = {commit_deadline}, network delay = {delta}, sync delay = 0"
    ));
    env = env
        .stop_cond(move |ctx| min_committed_height(ctx) > height_before)
        .deadline(commit_deadline);
    runner.run(&mut env).with_context(|| {
        format!(
            "no progress from epoch {epoch_view} by {commit_deadline}: \
             max height before = {height_before}, epoch entered at = {epoch_entered_at}, seed = {seed}"
        )
    })?;
    Ok(())
}

#[test]
fn epoch_boundary_waits_for_tc_test() -> Result<()> {
    let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT).build()?;
    let make_env = || {
        // Keep QC formation out of this scenario so every view ends by its timer.
        let delivery = |_: &Ctx, packet: &Packet| match packet.kind() {
            Kind::Proposal | Kind::Vote => Fate::Drop,
            _ => Fate::Delay(DELTA),
        };
        Env::new()
            .adversary(Adversary::new([]).delivery(delivery))
            .invariant(safety())
    };
    let mut runner = Runner::new(setup);

    let mut env = make_env().stop_cond(time_reached(VIEW_TIMEOUT));
    runner.run(&mut env)?;
    ensure!(
        min_view(&runner.context(&env)) == 1 && max_view(&runner.context(&env)) == 1,
        "view 1 occupies the second slot of the initial epoch"
    );

    let mut env = make_env().stop_cond(time_reached(2 * VIEW_TIMEOUT));
    runner.run(&mut env)?;
    ensure!(
        min_view(&runner.context(&env)) == 2 && max_view(&runner.context(&env)) == 2,
        "the timeout of view 1 must advance directly to view 2"
    );

    let mut env = make_env().stop_cond(time_reached(3 * VIEW_TIMEOUT));
    runner.run(&mut env)?;
    ensure!(
        min_view(&runner.context(&env)) == 3 && max_view(&runner.context(&env)) == 3,
        "the timeout of view 2 must advance directly to view 3"
    );

    let mut env = make_env().stop_cond(time_reached(4 * VIEW_TIMEOUT));
    runner.run(&mut env)?;
    ensure!(
        min_view(&runner.context(&env)) == 3 && max_view(&runner.context(&env)) == 3,
        "nodes must wait in view 3 for TC(4)"
    );

    let mut env = make_env()
        .stop_cond(|ctx| min_view(ctx) >= 4)
        .deadline(4 * VIEW_TIMEOUT + 2 * DELTA);
    runner.run(&mut env)?;
    ensure!(
        max_view(&runner.context(&env)) == 4,
        "TC(4) must enter view 4"
    );
    Ok(())
}

#[test]
fn epoch_qc_relay_synchronizes_honest_nodes_test() -> Result<()> {
    for_each_seed(&random_seeds(100), epoch_qc_relay_synchronizes_honest_nodes)
}

fn epoch_qc_relay_synchronizes_honest_nodes(seed: u64, verbose: bool) -> Result<()> {
    let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
        .rng(Randomness::new(seed))
        .verbose(verbose)
        .build()?;
    let adversary =
        Adversary::new([0]).byzantine_node(NodeSelector::Id(NodeID::from(0)), move || {
            ByzantineProxy::new(
                "reveal_qc_to_one",
                |_: &Context<'_, VirtualClock>| true,
                reveal_qc_to_one(4, NodeID::from(1)),
            )
        });
    let mut runner = Runner::new(setup);
    let mut env = Env::new()
        .adversary(adversary)
        .invariant(safety())
        .invariant(no_view_timeouts())
        .stop_cond(|ctx| max_view(ctx) >= 4)
        .deadline(7 * DELTA);
    runner.run(&mut env)?;
    let first_entered_at = runner.state().now();
    ensure!(
        runner.context(&env).honest_views().collect::<Vec<_>>()
            == vec![
                (NodeID::from(1), 4),
                (NodeID::from(2), 3),
                (NodeID::from(3), 3)
            ],
        "the QC must initially reach only n1"
    );

    // The collector stays silent while honest nodes disseminate QC(3).
    let silent = Adversary::new([0]).byzantine_node(NodeSelector::Id(NodeID::from(0)), move || {
        ByzantineProxy::new(
            "silent",
            |_: &Context<'_, VirtualClock>| true,
            |_, _| Ok(no_effects()),
        )
    });
    let mut env = Env::new()
        .adversary(silent)
        .invariant(safety())
        .invariant(no_view_timeouts())
        .stop_cond(|ctx| min_view(ctx) >= 4)
        .deadline(first_entered_at + DELTA);
    runner.run(&mut env)?;
    ensure!(
        max_view(&runner.context(&env)) == 4,
        "all honest nodes must be in view 4"
    );
    Ok(())
}

#[test]
fn sync_happy_path_test() -> Result<()> {
    let seeds = random_seeds(1_000);
    for_each_seed(&seeds, sync_happy_path)
}

fn sync_happy_path(seed: u64, verbose: bool) -> Result<()> {
    const END: u64 = 2_000;
    let rnd = Randomness::new(seed);
    let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let mut runner = Runner::new(setup);
    let mut env = Env::new()
        .invariant(safety())
        .invariant(no_k_view_timeouts(1))
        .stop_cond(time_reached(END));
    runner.run(&mut env)?;

    let min_commited_height = min_committed_height(&runner.context(&env));
    let want_commited_height = min_view(&runner.context(&env)) - 2;
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
    let rnd = Randomness::new(seed);
    let setup = setup(N4, DELTA, VIEW_TIMEOUT, LOCK_TIMEOUT)
        .rng(rnd.clone())
        .start_in_interval(&rnd, 0, DELTA - 1)
        .verbose(verbose)
        .build()?;

    let mut runner = Runner::new(setup);
    let mut env = Env::new()
        .invariant(safety())
        .invariant(no_k_view_timeouts(1))
        .stop_cond(time_reached(END));
    runner.run(&mut env)?;

    let min_commited_height = min_committed_height(&runner.context(&env));
    let want_commited_height = min_view(&runner.context(&env)) - 2;
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
    let rnd = Randomness::new(seed);
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

    let mut env = Env::new().invariant(safety()).stop_cond(time_reached(gst));
    runner.run(&mut env)?;
    runner.mark(gst, "GST");

    let view_at_gst = max_view(&runner.context(&env));
    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    runner.note(format!(
        "view_at_gst = {view_at_gst}, epoch_view = {epoch_view}"
    ));
    let mut env = Env::new()
        .invariant(safety())
        .stop_cond(move |ctx| max_view(ctx) >= epoch_view);
    runner.run(&mut env).with_context(at_gst_msg)?;

    let first_entered_at = runner.state().now();
    runner.note(format!("first_entered_at = {first_entered_at}"));
    runner.mark(first_entered_at, &format!("first in v{epoch_view}"));
    // The first node enters on a TC or, as the leader, on a QC. A TC reaches the others within
    // DELTA. A leader missing the block of its QC must fetch it before proposing, so the others
    // get the Proposal after sync_delay + DELTA. The simulator delivers a sync as a single packet
    // under the same delay as any message, hence sync_delay = DELTA.
    let sync_delay = DELTA;
    let mut env = Env::new()
        .invariant(safety())
        .stop_cond(move |ctx| min_view(ctx) >= epoch_view)
        .deadline(first_entered_at + DELTA + sync_delay);
    runner.run(&mut env).with_context(at_gst_msg)?;
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
    let rnd = Randomness::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let gst = rnd.range(7 * view_timeout, 30 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let make_env = || {
        let adversary = Adversary::new([0]).delivery(isolate_node_from_all(3, gst, delta));
        Env::new().adversary(adversary).invariant(safety())
    };
    let make_silent_env = || {
        let adversary = Adversary::new([0])
            .delivery(isolate_node_from_all(3, gst, delta))
            .byzantine_node(NodeSelector::Id(NodeID::from(0)), move || {
                ByzantineProxy::new(
                    "silent",
                    |_: &Context<'_, VirtualClock>| true,
                    |_, _| Ok(no_effects()),
                )
            });
        Env::new().adversary(adversary).invariant(safety())
    };
    let at_gst_msg = || format!("gst = {gst}");

    let mut runner = Runner::new(setup);
    runner.note(format!("gst = {gst}"));

    let mut env = make_env().stop_cond(time_reached(gst));
    runner.run(&mut env)?;
    runner.mark(gst, "GST");

    let view_at_gst = max_view(&runner.context(&env));
    ensure!(
        min_view(&runner.context(&env)) < view_at_gst,
        "n3 does not lag at gst = {gst}"
    );

    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    runner.note(format!(
        "view_at_gst = {view_at_gst}, epoch_view = {epoch_view}"
    ));
    let mut env = make_silent_env().stop_cond(move |ctx| max_view(ctx) >= epoch_view);
    runner.run(&mut env).with_context(at_gst_msg)?;

    let first_entered_at = runner.state().now();
    runner.note(format!("first_entered_at = {first_entered_at}"));
    runner.mark(first_entered_at, &format!("first in v{epoch_view}"));
    // The first node enters on a TC or, as the leader, on a QC. A TC reaches the others within
    // delta. A leader missing the block of its QC must fetch it before proposing, so the others
    // get the Proposal after sync_delay + delta. The simulator delivers a sync as a single packet
    // under the same delay as any message, hence sync_delay = delta.
    let sync_delay = delta;
    let mut env = make_silent_env()
        .stop_cond(move |ctx| min_view(ctx) >= epoch_view)
        .deadline(first_entered_at + delta + sync_delay);
    runner.run(&mut env).with_context(at_gst_msg)?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));

    // From the last entry, allow τ + 8Δ for a new commit. The silent n0 leads epoch_view,
    // so n1 enters the next view within τ, waits for its lock timer (3Δ), then Proposal,
    // votes, Proposal, votes and Proposal take 5Δ.
    let all_entered_at = runner.state().now();
    let height = max_committed_height(&runner.context(&env));
    let mut env = make_silent_env()
        .stop_cond(move |ctx| min_committed_height(ctx) > height)
        .deadline(all_entered_at + view_timeout + lock_timeout + 5 * delta);
    runner.run(&mut env).with_context(|| {
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
    let rnd = Randomness::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let gst = rnd.range(view_timeout, 30 * view_timeout);
    let max_delay = rnd.range(delta, 5 * view_timeout);
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let make_env = || {
        let delivery = until_gst(gst, delta, uniform(0, max_delay, rnd.clone()));
        Env::new()
            .adversary(Adversary::new([]).delivery(delivery))
            .invariant(safety())
    };
    let at_gst_msg = || format!("gst = {gst}, max_delay = {max_delay}");

    let mut runner = Runner::new(setup);
    runner.note(at_gst_msg());

    let mut env = make_env().stop_cond(time_reached(gst));
    runner.run(&mut env)?;
    runner.mark(gst, "GST");

    let view_at_gst = max_view(&runner.context(&env));
    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    runner.note(format!(
        "view_at_gst = {view_at_gst}, epoch_view = {epoch_view}"
    ));
    let mut env = make_env().stop_cond(move |ctx| max_view(ctx) >= epoch_view);
    runner.run(&mut env).with_context(at_gst_msg)?;

    let first_entered_at = runner.state().now();
    runner.note(format!("first_entered_at = {first_entered_at}"));
    runner.mark(first_entered_at, &format!("first in v{epoch_view}"));
    // The first node enters on a TC or, as the leader, on a QC. A TC reaches the others within
    // delta. A leader missing the block of its QC must fetch it before proposing, so the others
    // get the Proposal after sync_delay + delta. The simulator delivers a sync as a single packet
    // under the same delay as any message, hence sync_delay = delta.
    let sync_delay = delta;
    let mut env = make_env()
        .stop_cond(move |ctx| min_view(ctx) >= epoch_view)
        .deadline(first_entered_at + delta + sync_delay);
    runner.run(&mut env).with_context(at_gst_msg)?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));

    // The leader of epoch_view may wait for its lock timer (3Δ). Then Proposal, votes and the next
    // Proposal take 3Δ, and blocks delayed before GST can cost two fetches: by the leader before it
    // proposes and by the voters before they vote.
    let entered_at = runner.state().now();
    let timeouts = runner.state().view_timeouts();
    let next_view = epoch_view + 1;
    let mut env = make_env()
        .stop_cond(move |ctx| min_view(ctx) >= next_view)
        .deadline(entered_at + lock_timeout + 3 * delta + 2 * sync_delay);
    runner.run(&mut env).with_context(at_gst_msg)?;

    // From here on votes and the next Proposal take 2Δ. Processing the Proposal commits the
    // block of two views before. At an epoch boundary a standalone QC can advance the view first.
    for view in epoch_view + 2..=epoch_view + VIEWS_AFTER_ENTRY {
        let entered_at = runner.state().now();
        let height = min_committed_height(&runner.context(&env));
        let mut env = make_env()
            .stop_cond(move |ctx| min_view(ctx) >= view && min_committed_height(ctx) > height)
            .deadline(entered_at + 2 * delta);
        runner.run(&mut env).with_context(at_gst_msg)?;
        let new_height = min_committed_height(&runner.context(&env));
        ensure!(
            new_height > height && new_height == max_committed_height(&runner.context(&env)),
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
    let rnd = Randomness::new(seed);
    let delta = 5;
    let view_timeout = rnd.range(10 * delta, 20 * delta);
    let lock_timeout = 3 * delta;
    let setup = setup(N4, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;

    let adversary = Adversary::new([3])
        .byzantine_node(NodeSelector::Id(NodeID::from(3)), move || {
            ByzantineProxy::new(
                "tail_fork",
                |_: &Context<'_, VirtualClock>| true,
                tail_fork(),
            )
        });
    let mut runner = Runner::new(setup);
    // The first 3Δ reach the first view of n2. In each cycle, the stale QC in n3's proposal
    // makes peers wait for the penultimate view's timer, at most (epoch_length - 1)τ after
    // epoch entry. Allow another 5Δ for QC collection and the following honest proposals.
    let cycle_timeout = (RoundRobin::new(N4).epoch_length() - 1) * view_timeout;
    let mut env = Env::new()
        .adversary(adversary)
        .invariant(safety())
        .stop_cond(|ctx| min_view(ctx) >= LAST_VIEW)
        .deadline(3 * delta + CYCLES * (cycle_timeout + 5 * delta));
    runner.run(&mut env)?;

    // Every view has one block. One block of n2 is dropped per cycle, and the blocks of the last
    // two views are not committed yet.
    let height = min_committed_height(&runner.context(&env));
    let expected_height = LAST_VIEW - CYCLES - 2;
    ensure!(
        height == expected_height && height == max_committed_height(&runner.context(&env)),
        "expected height {expected_height} on every node, min is {height}"
    );

    let expected = BTreeSet::from([0u32, 1, 3].map(NodeID::from));
    for id in runner.context(&env).honest_nodes() {
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
        .rng(Randomness::new(seed))
        .verbose(verbose)
        .build()?;
    let attack = Rc::new(Cell::new(None));
    let recorded_attack = Rc::clone(&attack);
    let adversary =
        Adversary::new([3]).byzantine_node(NodeSelector::Id(NodeID::from(3)), move || {
            let recorded_attack = Rc::clone(&recorded_attack);
            ByzantineProxy::new(
                "conflicting_proposal",
                |_: &Context<'_, VirtualClock>| true,
                conflicting_proposal(ATTACK_VIEW, move |protected, fork| {
                    recorded_attack.set(Some((protected, fork)));
                }),
            )
        });

    let mut runner = Runner::new(setup);
    let mut env = Env::new()
        .adversary(adversary)
        .invariant(safety())
        // The stale QC cannot advance peers to v3. Wait for their timers and the replay
        // of the buffered fork, so rejection cannot be explained by a wrong current view.
        .stop_cond(|ctx| min_view(ctx) >= ATTACK_VIEW)
        .deadline(3 * VIEW_TIMEOUT);
    runner.run(&mut env)?;
    let (protected, fork) = attack
        .get()
        .context("n3 never proposed the conflicting block")?;
    proposal_rejected(ATTACK_VIEW, fork, 2, (1, protected))(&runner.context(&env))?;
    runner.mark(runner.state().now(), "conflicting proposal rejected");

    let silent = Adversary::new([3]).byzantine_node(NodeSelector::Id(NodeID::from(3)), move || {
        ByzantineProxy::new(
            "silent",
            |_: &Context<'_, VirtualClock>| true,
            |_, _| Ok(no_effects()),
        )
    });
    let mut env = Env::new()
        .adversary(silent)
        .invariant(safety())
        .stop_cond(|ctx| min_committed_height(ctx) >= 2)
        .deadline(5 * VIEW_TIMEOUT);
    runner.run(&mut env)?;
    for (id, log) in runner.context(&env).honest_commit_logs() {
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
        .rng(Randomness::new(seed))
        .verbose(verbose)
        .build()?;

    let lagging = || partition([&[5, 6]], gst, delta);
    let mut runner = Runner::new(setup);
    let mut env = Env::new()
        .adversary(Adversary::new([0, 1]).delivery(lagging()))
        .invariant(safety())
        .stop_cond(time_reached(gst));
    runner.run(&mut env)?;
    runner.mark(gst, "GST");

    let leaders = RoundRobin::new(N7);
    let view_at_gst = max_view(&runner.context(&env));
    let epoch_view = leaders.epoch_start(view_at_gst) + leaders.epoch_length();
    let make_silent_env = || {
        let adversary = Adversary::new([0, 1])
            .delivery(lagging())
            .byzantine_node(NodeSelector::Id(NodeID::from(0)), move || {
                ByzantineProxy::new(
                    "silent",
                    |_: &Context<'_, VirtualClock>| true,
                    |_, _| Ok(no_effects()),
                )
            })
            .byzantine_node(NodeSelector::Id(NodeID::from(1)), move || {
                ByzantineProxy::new(
                    "silent",
                    |_: &Context<'_, VirtualClock>| true,
                    |_, _| Ok(no_effects()),
                )
            });
        Env::new().adversary(adversary).invariant(safety())
    };
    let mut env = make_silent_env().stop_cond(move |ctx| max_view(ctx) >= epoch_view);
    runner.run(&mut env)?;
    runner.mark(runner.state().now(), &format!("first in v{epoch_view}"));
    let mut env = make_silent_env().stop_cond(move |ctx| min_view(ctx) >= epoch_view);
    runner.run(&mut env)?;
    runner.mark(runner.state().now(), &format!("all in v{epoch_view}"));
    let mut env = make_silent_env().stop_cond(time_reached(end));
    runner.run(&mut env)
}


fn setup(n: u32, delta: u64, view_timeout: u64, lock_timeout: u64) -> Setup {
    Setup::new(n)
        .delay(delta)
        .timeouts(view_timeout, lock_timeout)
        .verbose(VERBOSE)
}
