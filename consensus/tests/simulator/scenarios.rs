use super::adversary::*;
use super::byzantine_behaivors::{equivocation_proxy, non_monotonic_proxy, silent_proxy};
use super::dsl::*;
use super::invariants::*;
use super::rng::Randomness;
use super::runner::{Env, Runner};
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};
use hotstuff_types::node::NodeID;
use std::cell::Cell;
use std::rc::Rc;

const VERBOSE: bool = false;

#[test]
fn replay_seed() -> Result<()> {
    let seed = 7617890157137703680;
    replay(seed, recovery_among_honest)
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
            let mut proxy = equivocation_proxy();
            proxy.activate_when = Box::new(with_probability::<VirtualClock>(70));
            proxy
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

#[test]
fn recovery_test() -> Result<()> {
    for_each_seed(&random_seeds(100), recovery)
}

fn recovery(seed: u64, verbose: bool) -> Result<()> {
    let rnd = Randomness::new(seed);
    let n = rnd.range(4, 10) as u32;
    let f = rnd.range(1, u64::from((n - 1) / 3)) as u32;
    let delta = rnd.range(5, 30);
    let blocksync_delta = delta;
    let d = delta + blocksync_delta;
    let gst = rnd.range(1, 50 * delta);
    let lock_timeout = rnd.range(3 * d + 1, 5 * d);
    let view_timeout = rnd.range(lock_timeout + 7 * d + 1, lock_timeout + 10 * d);
    let leaders = RoundRobin::new(n);
    let epoch_duration = leaders.epoch_length() * view_timeout;

    let byzantines = rnd.sample_exact(n, f);
    let k = rnd.range(0, u64::from(f - 1)) as usize;
    let (silent_nodes, crashing_nodes) = byzantines.split_at(k);
    let crash_deadline = gst + 10 * epoch_duration;
    let mut stops: Vec<_> = crashing_nodes
        .iter()
        .map(|&id| (rnd.range(1, crash_deadline), NodeID::from(id)))
        .collect();
    stops.sort_unstable();

    let setup = setup(n, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .start_in_interval(&rnd, 1, delta * 10)
        .verbose(verbose)
        .build()?;
    let silent = Rc::new(Cell::new(false));
    let mut adversary = Adversary::new(byzantines.iter().copied());
    for &id in silent_nodes {
        let activate = Rc::clone(&silent);
        adversary = adversary.byzantine_node(NodeSelector::Id(NodeID::from(id)), move || {
            let activate = Rc::clone(&activate);
            let mut proxy = silent_proxy();
            proxy.activate_when = Box::new(move |_| activate.get());
            proxy
        });
    }
    let delivery = until_gst(gst, delta, uniform(1, 100 * delta, rnd.clone()));
    let mut env = Env::new()
        .adversary(adversary.delivery(delivery))
        .invariant(safety());
    let mut runner = Runner::new(setup);
    runner.mark(gst, "GST");
    runner.note(format!(
        "gst = {gst}, silent = {silent_nodes:?}, stops = {stops:?}"
    ));

    let mut stopped = Vec::new();
    for (at, id) in stops {
        runner.run_until(at, &mut env)?;
        stopped.push((id, runner.stop_node(id)?));
    }

    let all_stopped_at = runner.state().now();
    silent.set(true);
    runner.mark(
        all_stopped_at,
        "all selected Byzantine nodes stopped; remaining Byzantine nodes silent",
    );
    let recovery_deadline = all_stopped_at + epoch_duration;
    let mut recoveries: Vec<_> = stopped
        .into_iter()
        .map(|(id, db)| (rnd.range(all_stopped_at + 1, recovery_deadline), id, db))
        .collect();
    recoveries.sort_unstable_by_key(|(at, id, _)| (*at, *id));
    runner.note(format!(
        "recoveries = {:?}, recovery deadline = {recovery_deadline}",
        recoveries
            .iter()
            .map(|(at, id, _)| (*at, *id))
            .collect::<Vec<_>>()
    ));
    for (at, id, db) in recoveries {
        runner.run_until(at, &mut env)?;
        runner.recover_node(id, db, &mut env)?;
    }

    runner.run_until(gst.max(runner.state().now()), &mut env)?;
    ensure!(
        runner.state().running().count() == n as usize,
        "not all stopped Byzantine nodes recovered"
    );
    let all_recovered_at = runner.state().now();
    runner.mark(
        all_recovered_at,
        "all stopped Byzantine nodes recovered after GST",
    );
    let epoch_view = leaders.epoch_end(max_view(&runner.context(&env))) + 1;
    let smallest_view = min_view(&runner.context(&env));
    let epochs_to_wait = (epoch_view - leaders.epoch_start(smallest_view)) / leaders.epoch_length();
    let epoch_deadline = all_recovered_at + epochs_to_wait * (epoch_duration + 2 * d);
    env = env
        .stop_cond(move |ctx| max_view(ctx) >= epoch_view)
        .deadline(epoch_deadline);
    runner.run(&mut env).with_context(|| {
        format!("no entry into epoch {epoch_view} after Byzantine recovery, seed = {seed}")
    })?;

    let epoch_entered_at = runner.state().now();
    let height_before = max_committed_height(&runner.context(&env));
    let commit_deadline = epoch_entered_at + epoch_duration + 2 * d;
    runner.mark(
        epoch_entered_at,
        &format!("first honest in epoch {epoch_view}"),
    );
    runner.note(format!(
        "epoch_view = {epoch_view}, height_before = {height_before}, \
         commit deadline = {commit_deadline}, network delay = {delta}, sync delay = {blocksync_delta}"
    ));
    let epoch_sync_deadline = epoch_entered_at + 2 * d;
    env = env.deadline(epoch_sync_deadline);
    runner.run_until(epoch_sync_deadline, &mut env)?;
    let smallest_view = min_view(&runner.context(&env));
    ensure!(
        smallest_view >= epoch_view,
        "not all honest nodes reached epoch {epoch_view} by {epoch_sync_deadline}: \
         minimum view = {smallest_view}, epoch entered at = {epoch_entered_at}, seed = {seed}"
    );
    runner.mark(
        epoch_sync_deadline,
        &format!("all honest crossed epoch {epoch_view}"),
    );
    env = env
        .stop_cond(move |ctx| min_committed_height(ctx) > height_before)
        .deadline(commit_deadline);
    runner.run(&mut env).with_context(|| {
        format!(
            "no progress after Byzantine recovery in epoch {epoch_view} by {commit_deadline}: \
             max height before = {height_before}, epoch entered at = {epoch_entered_at}, seed = {seed}"
        )
    })?;
    Ok(())
}

#[test]
#[should_panic(expected = "no events, no timers")]
fn recovery_among_honest_test() {
    for_each_seed(&random_seeds(1_0_000), recovery_among_honest).unwrap();
}

fn recovery_among_honest(seed: u64, verbose: bool) -> Result<()> {
    let rnd = Randomness::new(seed);
    let n = rnd.range(4, 10) as u32;
    let f = rnd.range(1, u64::from((n - 1) / 3)) as u32;
    let delta = rnd.range(5, 30);
    let blocksync_delta = delta;
    let d = delta + blocksync_delta;
    let gst = rnd.range(1, 50 * delta);
    let lock_timeout = rnd.range(3 * d + 1, 5 * d);
    let view_timeout = rnd.range(lock_timeout + 7 * d + 1, lock_timeout + 10 * d);
    let leaders = RoundRobin::new(n);
    let epoch_duration = leaders.epoch_length() * view_timeout;

    let byzantines = rnd.sample_exact(n, f);
    let honest: Vec<_> = (0..n).filter(|id| !byzantines.contains(id)).collect();
    let crash_count = rnd.range(1, honest.len() as u64) as u32;
    let crash_deadline = gst + 10 * epoch_duration;
    let mut stops: Vec<_> = rnd
        .sample_exact(honest.len() as u32, crash_count)
        .into_iter()
        .map(|index| {
            (
                rnd.range(1, crash_deadline),
                NodeID::from(honest[index as usize]),
            )
        })
        .collect();
    stops.sort_unstable();

    let setup = setup(n, delta, view_timeout, lock_timeout)
        .rng(rnd.clone())
        .verbose(verbose)
        .build()?;
    let silent = Rc::new(Cell::new(false));
    let activate = Rc::clone(&silent);
    let adversary = Adversary::new(byzantines).byzantine_node(NodeSelector::All, move || {
        let activate = Rc::clone(&activate);
        let mut proxy = silent_proxy();
        proxy.activate_when = Box::new(move |_| activate.get());
        proxy
    });
    let delivery = until_gst(gst, delta, uniform(1, 100 * delta, rnd.clone()));
    let mut env = Env::new()
        .adversary(adversary.delivery(delivery))
        .invariant(safety());
    let mut runner = Runner::new(setup);
    runner.mark(gst, "GST");
    runner.note(format!("gst = {gst}, stops = {stops:?}"));

    let mut stopped = Vec::new();
    for (at, id) in stops {
        runner.run_until(at, &mut env)?;
        stopped.push((id, runner.stop_node(id)?));
    }

    let all_stopped_at = runner.state().now();
    silent.set(true);
    runner.mark(
        all_stopped_at,
        "all selected honest stopped; Byzantine nodes silent",
    );
    let recovery_deadline = all_stopped_at + epoch_duration;
    let mut recoveries: Vec<_> = stopped
        .into_iter()
        .map(|(id, db)| (rnd.range(all_stopped_at + 1, recovery_deadline), id, db))
        .collect();
    recoveries.sort_unstable_by_key(|(at, id, _)| (*at, *id));
    runner.note(format!(
        "recoveries = {:?}, recovery deadline = {recovery_deadline}",
        recoveries
            .iter()
            .map(|(at, id, _)| (*at, *id))
            .collect::<Vec<_>>()
    ));
    for (at, id, db) in recoveries {
        runner.run_until(at, &mut env)?;
        runner.recover_node(id, db, &mut env)?;
    }

    runner.run_until(gst.max(runner.state().now()), &mut env)?;
    ensure!(
        runner.context(&env).honest_nodes().count() == (n - f) as usize,
        "not all honest nodes recovered"
    );
    let all_recovered_at = runner.state().now();
    runner.mark(all_recovered_at, "all honest recovered after GST");
    let epoch_view = leaders.epoch_end(max_view(&runner.context(&env))) + 1;
    let smallest_view = min_view(&runner.context(&env));
    let epochs_to_wait = (epoch_view - leaders.epoch_start(smallest_view)) / leaders.epoch_length();
    let epoch_deadline = all_recovered_at + epochs_to_wait * (epoch_duration + 2 * d);
    env = env
        .stop_cond(move |ctx| max_view(ctx) >= epoch_view)
        .deadline(epoch_deadline);
    runner.run(&mut env).with_context(|| {
        format!("no entry into epoch {epoch_view} after recovery, seed = {seed}")
    })?;

    let epoch_entered_at = runner.state().now();
    let height_before = max_committed_height(&runner.context(&env));
    let commit_deadline = epoch_entered_at + epoch_duration + 2 * d;
    runner.mark(
        epoch_entered_at,
        &format!("first honest in epoch {epoch_view}"),
    );
    runner.note(format!(
        "epoch_view = {epoch_view}, height_before = {height_before}, \
         commit deadline = {commit_deadline}, network delay = {delta}, sync delay = {blocksync_delta}"
    ));
    env = env
        .stop_cond(move |ctx| min_committed_height(ctx) > height_before)
        .deadline(commit_deadline);
    runner.run(&mut env).with_context(|| {
        format!(
            "no progress after recovery in epoch {epoch_view} by {commit_deadline}: \
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
