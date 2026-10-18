use crate::types::View;
use crate::types::vote::Vote;
use codec::{Decode, Encode};
use hotstuff_crypto::sign::{Certificate as RawCertificate, PartialSignature, Scheme};
use hotstuff_types::node::NodeID;
use hotstuff_types::threshold::Quorum;
use crate::types::wish::Wish;

pub type BlockQC<S> = QuorumCertificate<S, Vote<S>>;

pub type TimeoutQC<S> = QuorumCertificate<S, Wish>;

#[derive(Encode, Decode)]
pub struct QuorumCertificate<S: Scheme, T> {
    pub subject: T,
    pub raw: RawCertificate<S::Digest, Quorum, S::Aggregate>,
}


impl<S: Scheme> BlockQC<S> {
    pub fn view(&self) -> View {
        self.subject.view
    }

    pub fn block(&self) -> S::Digest {
        self.subject.block
    }
}

impl<S: Scheme> TimeoutQC<S> {
    pub fn view(&self) -> View {
        self.subject.view
    }
}

impl<S: Scheme, T: Clone> Clone for QuorumCertificate<S, T> {
    fn clone(&self) -> Self {
        Self {
            subject: self.subject.clone(),
            raw: self.raw.clone(),
        }
    }
}

pub type SignedVote<S> = Signature<S, Vote<S>>;

impl<S: Scheme> SignedVote<S> {
    pub fn view(&self) -> View {
        self.subject.view
    }

    pub fn block_hash(&self) -> S::Digest {
        self.subject.block
    }
}

pub type SignedWish<S> = Signature<S, Wish>;

impl<S: Scheme> SignedWish<S> {
    pub fn view(&self) -> View {
        self.subject.view
    }
}

#[derive(Encode, Decode)]
pub struct Signature<S: Scheme, T> {
    pub subject: T,
    pub raw: PartialSignature<S::Signature>,
}

impl <S: Scheme, T> Signature<S, T> {
    pub fn creator(&self) -> NodeID{
        self.raw.node_id()
    }
}

impl<S: Scheme, T: Clone> Clone for Signature<S, T> {
    fn clone(&self) -> Self {
        Self {
            subject: self.subject.clone(),
            raw: self.raw.clone(),
        }
    }
}


