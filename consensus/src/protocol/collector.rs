use std::marker::PhantomData;
use crate::types::View;
use crate::types::certificate::{CertifiedBlock, CertifiedView};
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

pub enum CollectorEvent<S: Scheme> {
    GenericVoteInput { sender: NodeID, vote: Vote<S> },
    WishInput { sender: NodeID, view: View },
}

pub enum CollectorOutput<S: Scheme> {
    GenericOutput(GenericCollectorOutput<S>),
    TimeoutOutput(TimeoutCollectorOutput<S>),
}

pub enum GenericCollectorOutput<S: Scheme> {
    Ready(CertifiedBlock<S>),
    AlreadyReady(View),
    NotReady,
}

pub enum TimeoutCollectorOutput<S: Scheme> {
    TimeoutReady(CertifiedView<S>),
    TimeoutAlreadyReady(View),
    TimeoutNotReady,
}

pub struct Collector<S: Scheme> {
    _scheme: PhantomData<S>,
}

impl<S: Scheme> Collector<S> {
    pub fn apply(&mut self, event: CollectorEvent<S>) -> CollectorOutput<S> {
        match event {
            CollectorEvent::GenericVoteInput { sender, vote } => CollectorOutput::GenericOutput(self.add_generic_vote(sender, vote)),
            CollectorEvent::WishInput { sender, view } => CollectorOutput::TimeoutOutput(self.add_wish(sender, view)),
        }
    }

    fn add_generic_vote(&mut self, sender: NodeID, vote: Vote<S>) -> GenericCollectorOutput<S> {
        todo!()
    }

    fn add_wish(&mut self, sender: NodeID, view: View) -> TimeoutCollectorOutput<S> {
        todo!()
    }
}
