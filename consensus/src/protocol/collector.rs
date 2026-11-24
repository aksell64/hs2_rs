use crate::protocol::collector::CertificateOutput::{AlreadyReady, NotReady, Ready};
use crate::storage::{KVError, ReadWriteStorage};
use crate::types::View;
use crate::types::block::Command;
use crate::types::certificate::{QuorumCertificate, Signature, SignedVote, SignedWish};
use crate::types::crypto::{Notary, Signable};
use crate::types::vote::Vote;
use crate::types::wish::Wish;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::threshold::{Quorum, Threshold};

pub enum CollectorInput<S: Scheme> {
    GenericVoteInput(SignedVote<S>),
    WishInput(SignedWish<S>),
}

pub enum CollectorOutput<S: Scheme> {
    GenericOutput(GenericCollectorOutput<S>),
    TimeoutOutput(TimeoutCollectorOutput<S>),
}

pub enum CertificateOutput<S: Scheme, T> {
    Ready(QuorumCertificate<S, T>),
    AlreadyReady(View),
    NotReady,
}

pub type GenericCollectorOutput<S> = CertificateOutput<S, Vote<S>>;

pub type TimeoutCollectorOutput<S> = CertificateOutput<S, Wish>;

pub struct Collector<S: Scheme> {
    notary: Notary<S>,
    threshold: u32,
}

impl<S: Scheme> Collector<S> {
    pub fn new(notary: Notary<S>) -> Self {
        Self {
            threshold: Quorum::required(notary.committee_size()),
            notary,
        }
    }

    pub fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        event: CollectorInput<S>,
    ) -> Result<CollectorOutput<S>, KVError> {
        let output = match event {
            CollectorInput::GenericVoteInput(vote) => {
                CollectorOutput::GenericOutput(self.on_vote(storage, vote)?)
            }
            CollectorInput::WishInput(wish) => {
                CollectorOutput::TimeoutOutput(self.on_wish(storage, wish)?)
            }
        };
        Ok(output)
    }

    fn on_vote<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        vote: SignedVote<S>,
    ) -> Result<GenericCollectorOutput<S>, KVError> {
        let view = vote.view();
        if storage.get_block_qc(view)?.is_some() {
            return Ok(AlreadyReady(view));
        }
        if storage.exists_vote(view, vote.creator())? {
            return Ok(NotReady);
        }
        storage.insert_vote(&vote)?;
        if storage.get_votes_count(view, &vote.block_hash())? < self.threshold as usize {
            return Ok(NotReady);
        }
        let Some(qc) = self.aggregate(vote.subject, storage.get_votes(view)?) else {
            return Ok(NotReady);
        };
        storage.insert_block_qc(&qc)?;
        Ok(Ready(qc))
    }

    fn on_wish<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        wish: SignedWish<S>,
    ) -> Result<TimeoutCollectorOutput<S>, KVError> {
        let view = wish.view();
        if storage.get_timeout_qc(view)?.is_some() {
            return Ok(AlreadyReady(view));
        }
        if storage.exists_wish_vote(view, wish.creator())? {
            return Ok(NotReady);
        }
        storage.insert_wish_vote(&wish)?;
        if storage.get_wish_votes_count(view)? < self.threshold as usize {
            return Ok(NotReady);
        }
        let Some(qc) = self.aggregate(wish.subject, storage.get_wish_votes(view)?) else {
            return Ok(NotReady);
        };
        storage.insert_timeout_qc(&qc)?;
        Ok(Ready(qc))
    }

    fn aggregate<T: Signable + PartialEq>(
        &self,
        subject: T,
        signatures: Vec<Signature<S, T>>,
    ) -> Option<QuorumCertificate<S, T>> {
        let partials: Vec<_> = signatures
            .into_iter()
            .filter(|signature| signature.subject == subject)
            .map(|signature| signature.raw)
            .collect();
        if partials.len() < self.threshold as usize {
            return None;
        }
        self.notary.aggregate(subject, partials.into_iter()).ok()
    }
}
