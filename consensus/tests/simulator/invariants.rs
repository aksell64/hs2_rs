use crate::simulator::dsl::State;
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::storage::ReadStorage;
use hotstuff_consensus::types::View;
use hotstuff_types::node::NodeID;
use std::collections::BTreeMap;

pub fn safety() -> impl FnMut(&State) -> Result<()> {
    let mut canonical: BTreeMap<u64, TestDigest> = BTreeMap::new();
    let mut checked: BTreeMap<NodeID, usize> = BTreeMap::new();
    move |state| {
        for (id, log) in state.honest_commit_logs() {
            let done = checked.entry(id).or_insert(0);
            for i in *done..log.len() {
                let commit = &log[i];
                if let Some(prev) = i.checked_sub(1).map(|j| &log[j]) {
                    ensure!(
                        commit.height == prev.height + 1,
                        "{id:?} committed height {} right after {}",
                        commit.height,
                        prev.height
                    );
                }
                let hash = *canonical.entry(commit.height).or_insert(commit.hash);
                ensure!(
                    hash == commit.hash,
                    "{id:?} committed {:?} at height {}, but {hash:?} was committed there before",
                    commit.hash,
                    commit.height
                );
            }
            *done = log.len();
        }
        Ok(())
    }
}

/// Check a completed proposal attempt: all honest replicas processed it without changing
/// their last vote or lock. Call at the checkpoint, before advancing to subsequent views.
pub fn proposal_rejected(
    view: View,
    proposal: TestDigest,
    voted_view: View,
    locked: (View, TestDigest),
) -> impl FnMut(&State) -> Result<()> {
    move |state| {
        for id in state.honest_nodes() {
            let replica = state.replica(id).context("honest node is not running")?;
            let storage: &dyn ReadStorage<TestScheme, TestCommand> = replica.storage();
            ensure!(
                storage.get_cur_view()? == view,
                "{id:?} is not in proposal view {view}"
            );
            // A structurally valid but rejected proposal is stored by the driver. Its presence
            // proves that the vote rule was reached, rather than an earlier validation rejecting it.
            ensure!(
                storage.get_block_by_hash(&proposal)?.is_some(),
                "{id:?} did not process proposal {proposal:?}"
            );
            ensure!(
                storage.get_voted_view()? == voted_view,
                "{id:?} did not retain voted view {voted_view}"
            );
            let lock = storage.get_locked_qc()?;
            ensure!(
                (lock.view(), lock.block()) == locked,
                "{id:?} did not retain lock {locked:?}"
            );
        }
        Ok(())
    }
}

pub fn no_view_timeouts() -> impl FnMut(&State) -> Result<()> {
    |state| {
        ensure!(
            state.view_timeouts() == 0,
            "{} view timers fired",
            state.view_timeouts()
        );
        Ok(())
    }
}

pub fn no_k_view_timeouts(k: u64) -> impl FnMut(&State) -> Result<()> {
    move |state| {
        ensure!(
            state.view_timeouts() <= k,
            "{} view timers fired",
            state.view_timeouts()
        );
        Ok(())
    }
}
