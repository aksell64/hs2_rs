use codec::Encode;
use crate::types::certificate::QuorumCertificate;
use hotstuff_crypto::sign::{AggregateSignature, Digest};

pub struct Block<D: Digest, C: Command, A: AggregateSignature> {
    pub parent: D,
    pub cmd: C,
    pub justify: QuorumCertificate<D, A>,
    pub height: u64,
    pub hash: Option<D>,
}

pub trait Command: Encode {}

