use crate::types::View;
use crate::types::vote::Vote;
use codec::{Decode, Encode};
use hotstuff_crypto::sign::{Certificate as RawCertificate, PartialSignature, Scheme};
use hotstuff_types::threshold::Quorum;

pub type BlockQC<S> = QuorumCertificate<S, Vote<S>>;

pub type TimeoutQC<S> = QuorumCertificate<S, View>;

#[derive(Encode, Decode)]
pub struct QuorumCertificate<S: Scheme, T> {
    pub subject: T,
    pub raw: RawCertificate<S::Digest, Quorum, S::Aggregate>,
}

#[derive(Encode, Decode)]
pub struct Signature<S: Scheme, T> {
    pub subject: T,
    pub raw: PartialSignature<S::Signature>,
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
        self.subject
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

impl<S: Scheme, T: Clone> Clone for Signature<S, T> {
    fn clone(&self) -> Self {
        Self {
            subject: self.subject.clone(),
            raw: self.raw.clone(),
        }
    }
}
