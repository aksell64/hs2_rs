use crate::protocol::driver::DriverInput;
use crate::types::View;
use crate::types::block::Command;
use hotstuff_crypto::sign::Scheme;
use std::collections::{BTreeMap, HashMap};

pub(super) struct InputKeeper<S: Scheme, C: Command> {
    by_view: BTreeMap<View, Vec<DriverInput<S, C>>>,
    by_block: HashMap<S::Digest, Vec<DriverInput<S, C>>>,
}

impl<S: Scheme, C: Command> InputKeeper<S, C> {
    pub(super) fn new() -> Self {
        Self {
            by_block: HashMap::new(),
            by_view: BTreeMap::new(),
        }
    }

    pub(super) fn buffer_for_view(&mut self, view: View, input: DriverInput<S, C>) {
        self.by_view.entry(view).or_default().push(input);
    }

    pub(super) fn buffer_for_block(&mut self, digest: S::Digest, input: DriverInput<S, C>) {
        self.by_block.entry(digest).or_default().push(input);
    }

    pub(super) fn release_view(&mut self, view: View) -> Vec<DriverInput<S, C>> {
        self.by_view
            .extract_if(..=view, |_, _| true)
            .flat_map(|(_, inputs)| inputs)
            .collect()
    }

    pub(super) fn release_block(&mut self, digest: S::Digest) -> Vec<DriverInput<S, C>> {
        self.by_block.remove(&digest).unwrap_or_default()
    }
}
