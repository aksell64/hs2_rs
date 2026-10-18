use hotstuff_crypto::sign::Scheme;
use crate::types::block::{Block, Command};
use crate::types::certificate::BlockQC;

struct Genesis<S: Scheme, C: Command>{
    pub block: Block<S, C>,
    pub qc: BlockQC<S>
}