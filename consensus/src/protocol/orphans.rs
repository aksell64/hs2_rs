use crate::protocol::consensus::ProposeRequest;
use crate::types::View;
use crate::types::block::{Command, Proposal};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::HashMap;

pub enum Deferred<S: Scheme, C: Command> {
    Propose(ProposeRequest<C>),
    ReceiveProposal {
        sender: NodeID,
        proposal: Proposal<S, C>,
    },
}

pub struct Orphans<S: Scheme, C: Command> {
    // missing -> deferred
    missing: HashMap<S::Digest, Vec<Deferred<S, C>>>,
}

impl<S: Scheme, C: Command> Orphans<S, C> {
    pub fn new() -> Self {
        Self {
            missing: HashMap::new(),
        }
    }

    pub fn add_deferred(&mut self, missing: S::Digest, deferred: Deferred<S, C>) {
        self.missing.entry(missing).or_default().push(deferred);
    }

    pub fn pop_deferred(&mut self, digest: &S::Digest) -> Vec<Deferred<S, C>> {
        self.missing.remove(digest).unwrap_or_default()
    }
}
