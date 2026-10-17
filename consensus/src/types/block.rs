use codec::{Decode, Encode};
use crate::types::View;
use crate::types::certificate::BlockQC;
use hotstuff_crypto::sign::Scheme;

#[derive(Encode, Decode)]
pub struct Block<S: Scheme, C: Command> {
    pub parent: S::Digest,
    pub cmd: C,
    pub justify: BlockQC<S>,
    pub height: u64,
    pub hash: S::Digest,
}

pub trait Command: Encode + Decode + Clone {}

#[derive(Encode, Decode)]
pub struct Proposal<S: Scheme, C: Command> {
    pub block: Block<S, C>,
    pub view: View,
}

impl<S: Scheme, C: Command> Clone for Block<S, C> {
    fn clone(&self) -> Self {
        Self {
            parent: self.parent,
            cmd: self.cmd.clone(),
            justify: self.justify.clone(),
            height: self.height,
            hash: self.hash,
        }
    }
}

impl<S: Scheme, C: Command> Clone for Proposal<S, C> {
    fn clone(&self) -> Self {
        Self {
            block: self.block.clone(),
            view: self.view,
        }
    }
}
