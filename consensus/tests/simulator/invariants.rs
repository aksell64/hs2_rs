use crate::simulator::dsl::State;
use anyhow::{Result, ensure};
use hotstuff_consensus::mocks::scheme::TestDigest;
use hotstuff_types::node::NodeID;
use std::collections::BTreeMap;

pub fn safety() -> impl FnMut(&State) -> Result<()> {
    let mut canonical: BTreeMap<u64, TestDigest> = BTreeMap::new();
    let mut checked: BTreeMap<NodeID, usize> = BTreeMap::new();
    move |state| {
        for (id, log) in state.commit_logs() {
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
