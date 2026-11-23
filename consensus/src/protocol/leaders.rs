use crate::types::View;
use hotstuff_types::node::NodeID;
use std::collections::BTreeSet;

pub trait LeaderPolicy: Send + 'static {
    fn leader(&self, view: View) -> NodeID;

    fn epoch_length(&self) -> View;

    fn epoch_of(&self, view: View) -> View {
        view / self.epoch_length()
    }

    fn epoch_start(&self, view: View) -> View {
        view - view % self.epoch_length()
    }

    fn epoch_end(&self, view: View) -> View {
        self.epoch_start(view) + self.epoch_length() - 1
    }

    fn is_epoch_start(&self, view: View) -> bool {
        view % self.epoch_length() == 0
    }

    fn epoch_leaders(&self, view: View) -> BTreeSet<NodeID> {
        (self.epoch_start(view)..=self.epoch_end(view))
            .map(|v| self.leader(v))
            .collect()
    }
}

pub struct RoundRobin {
    committee_size: u32,
}

impl RoundRobin {
    pub fn new(committee_size: u32) -> Self {
        Self { committee_size }
    }
}

impl LeaderPolicy for RoundRobin {
    fn leader(&self, view: View) -> NodeID {
        NodeID::from((view % View::from(self.committee_size)) as u32)
    }

    fn epoch_length(&self) -> View {
        View::from(self.committee_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_epoch_contains_the_whole_committee() {
        for n in [1, 4, 5, 7, 10] {
            let policy = RoundRobin::new(n);
            let committee = (0..n).map(NodeID::from).collect::<BTreeSet<_>>();
            assert_eq!(policy.epoch_length(), View::from(n));
            for view in 0..View::from(3 * n) {
                assert_eq!(
                    policy.epoch_leaders(view),
                    committee,
                    "n = {n}, view = {view}"
                );
            }
        }
    }
}
