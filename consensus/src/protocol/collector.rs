use std::marker::PhantomData;
use crate::types::View;
use crate::types::certificate::{BlockQC, TimeoutQC};
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

pub enum CollectorInput<S: Scheme> {
    GenericVoteInput { sender: NodeID, vote: Vote<S> },
    WishInput { sender: NodeID, view: View },
}

pub enum CollectorOutput<S: Scheme> {
    GenericOutput(GenericCollectorOutput<S>),
    TimeoutOutput(TimeoutCollectorOutput<S>),
}

pub enum GenericCollectorOutput<S: Scheme> {
    Ready(BlockQC<S>),
    AlreadyReady(View),
    NotReady,
}

pub enum TimeoutCollectorOutput<S: Scheme> {
    TimeoutReady(TimeoutQC<S>),
    TimeoutAlreadyReady(View),
    TimeoutNotReady,
}

pub struct Collector<S: Scheme> {
    _scheme: PhantomData<S>,
}

impl<S: Scheme> Collector<S> {
    pub fn apply(&mut self, event: CollectorInput<S>) -> CollectorOutput<S> {
        match event {
            CollectorInput::GenericVoteInput { sender, vote } => CollectorOutput::GenericOutput(self.add_generic_vote(sender, vote)),
            CollectorInput::WishInput { sender, view } => CollectorOutput::TimeoutOutput(self.add_wish(sender, view)),
        }
    }

    fn add_generic_vote(&mut self, sender: NodeID, vote: Vote<S>) -> GenericCollectorOutput<S> {
        todo!()
    }

    fn add_wish(&mut self, sender: NodeID, view: View) -> TimeoutCollectorOutput<S> {
        todo!()
    }
}
