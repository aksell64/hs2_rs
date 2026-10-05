use std::collections::BTreeSet;
use hotstuff_types::node::NodeID;

struct ValidatorsCommittee {
    set: BTreeSet<NodeID>,
}

impl ValidatorsCommittee {
    fn is_validator(&self, node_id: NodeID) -> bool {
        self.set.contains(&node_id)
    }
}

struct Validator {
    node_id: NodeID,
}
