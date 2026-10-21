use crate::protocol::consensus::ConsensusInput;
use crate::storage::ReadWriteStorage;
use crate::types::View;
use crate::types::block::Command;
use crate::types::certificate::BlockQC;
use hotstuff_crypto::sign::Scheme;

trait Consensus<S: Scheme> {
    fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
            &self,
            storage: &mut DB,
        cur_view: View,
        high_qc: &BlockQC<S>,
        event: ConsensusInput<S, C>,
    );
}
