use crate::protocol::collector::{CollectorInput, CollectorOutput};
use crate::protocol::consensus::{ConsensusInput, ConsensusResult};
use crate::protocol::pacemaker::{PacemakerInput, PacemakerResult};
use crate::storage::{KVError, ReadWriteStorage};
use crate::types::View;
use crate::types::block::Command;
use crate::types::certificate::BlockQC;
use crate::types::clock::Clock;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

pub trait Consensus<S: Scheme> {
    fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        cur_view: View,
        high_qc: &BlockQC<S>,
        event: ConsensusInput<S, C>,
    ) -> ConsensusResult<S, C>;
}

pub trait Pacemaker<S: Scheme> {
    type Clock: Clock;

    fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        event: PacemakerInput<S>,
    ) -> Result<PacemakerResult<S, C, Self::Clock>, KVError>;

    fn leader(&self, view: View) -> NodeID;
    fn is_leader(&self, view: View) -> bool;
    fn is_epoch_leader(&self, view: View) -> bool;
}

pub trait Collector<S: Scheme> {
    fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        event: CollectorInput<S>,
    ) -> Result<CollectorOutput<S>, KVError>;
}
