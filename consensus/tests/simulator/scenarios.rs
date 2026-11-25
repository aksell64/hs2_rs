use super::adversary::*;
use super::byzantine::{
    ByzantineProxy, conflicting_proposal, equivocation as equivocation_behavior, reveal_qc_to_one,
    tail_fork,
};
use super::byzantine_behaivors::{no_lock_timer_proxy, non_monotonic_proxy};
use super::dsl::*;
use super::invariants::*;
use super::rng::Randomness;
use super::runner::{Env, Runner};
use super::simulator::{Context, no_effects};
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::network::message::{Message, Recipient};
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
    replay(seed, non_monotonic_votes)
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
    let blocksync_delta = delta;
    let gst = rnd.range(1, 50 * delta);
    let lock_timeout = rnd.range(
        3 * (delta + blocksync_delta) + 1,
        5 * (delta + blocksync_delta),
    );
    let view_timeout = rnd.range(
        lock_timeout + 7 * (delta + blocksync_delta) + 1,
        lock_timeout + 10 * (delta + blocksync_delta),
    );

    let setup = setup(n, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .start_in_interval(&rnd, 1, delta * 10)
        .build()?;

    let adversary =
        Adversary::new(rnd.sample_exact(n, f)).byzantine_node(NodeSelector::Range(f), || {
            ByzantineProxy::new(
                "eviction",
                with_probability::<VirtualClock>(70),
                equivocation_behavior(),
            )
        });

    let delivery = until_gst(gst, delta, uniform(1, 100 * delta, rnd.clone()));

    let mut runner = Runner::new(setup);

    let mut env = Env::new()
        .adversary(adversary.delivery(delivery))
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
    // Use the L6–L7 bound with d = delta + blocksync_delta:
    // (n - 1)τ + LT + 9d < nτ + 2d, even across the epoch boundary.
    let commit_deadline =
        epoch_entered_at + leaders.epoch_length() * view_timeout + 2 * (delta + blocksync_delta);
    runner.mark(
        epoch_entered_at,
        &format!("first honest in epoch {epoch_view}"),
    );
    runner.note(format!(
        "commit deadline = {commit_deadline}, network delay = {delta}, sync delay = {blocksync_delta}"
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
fn non_monotonic_votes_test() -> Result<()> {
    for_each_seed(&random_seeds(1_0_000), non_monotonic_votes)
}

fn non_monotonic_votes(seed: u64, verbose: bool) -> Result<()> {
    let rnd = Randomness::new(seed);
    let n = rnd.range(4, 10) as u32;
    let f = rnd.range(1, u64::from((n - 1) / 3)) as u32;
    let delta = rnd.range(5, 30);
    let blocksync_delta = delta;
    let gst = rnd.range(500, 500 * delta);
    let lock_timeout = rnd.range(
        3 * (delta + blocksync_delta) + 1,
        5 * (delta + blocksync_delta),
    );
    let view_timeout = rnd.range(
        lock_timeout + 7 * (delta + blocksync_delta) + 1,
        lock_timeout + 10 * (delta + blocksync_delta),
    );

    let setup = setup(n, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .start_in_interval(&rnd, 1, delta * 10)
        .build()?;

    let clock = setup.state().clock().clone();
    let adversary =
        Adversary::new(rnd.sample_exact(n, f)).byzantine_node(NodeSelector::Range(f), move || {
            let mut proxy = non_monotonic_proxy(
                clock.clone(),
                FixedPolicy {
                    view_timeout,
                    lock_timeout,
                },
            );
            proxy.activate_when = Box::new(with_probability::<VirtualClock>(70));
            proxy
        });

    let delivery = until_gst(gst, delta, uniform(1, 10000 * delta, rnd.clone()));
    let mut runner = Runner::new(setup);
    let mut env = Env::new()
        .adversary(adversary.delivery(delivery))
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
    let commit_deadline =
        epoch_entered_at + leaders.epoch_length() * view_timeout + 2 * (delta + blocksync_delta);
    runner.mark(
        epoch_entered_at,
        &format!("first honest in epoch {epoch_view}"),
    );
    runner.note(format!(
        "commit deadline = {commit_deadline}, network delay = {delta}, sync delay = {blocksync_delta}"
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

fn setup(n: u32, delta: u64, view_timeout: u64, lock_timeout: u64) -> Setup {
    Setup::new(n)
        .delay(delta)
        .timeouts(view_timeout, lock_timeout)
        .verbose(VERBOSE)
}
