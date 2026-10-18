use crate::protocol::collector::CertificateOutput::{AlreadyReady, NotReady, Ready};
use crate::types::View;
use crate::types::certificate::{QuorumCertificate, Signature, SignedVote, SignedWish};
use crate::types::crypto::{Notary, Signable};
use crate::types::vote::Vote;
use crate::types::wish::Wish;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use hotstuff_types::threshold::{Quorum, Threshold};
use std::collections::BTreeMap;

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
    generic_collectors: BTreeMap<View, CertificateCollector<S, Vote<S>>>,
    timeout_collectors: BTreeMap<View, CertificateCollector<S, Wish>>,
    threshold: u32,
}

impl<S: Scheme> Collector<S> {
    pub fn new(notary: Notary<S>) -> Self {
        Self {
            threshold: Quorum::required(notary.committee_size()),
            notary,
            generic_collectors: BTreeMap::new(),
            timeout_collectors: BTreeMap::new(),
        }
    }

    pub fn apply(&mut self, event: CollectorInput<S>) -> CollectorOutput<S> {
        match event {
            CollectorInput::GenericVoteInput(vote) => CollectorOutput::GenericOutput(Self::add_signature(
                &mut self.generic_collectors,
                &self.notary,
                self.threshold,
                vote.view(),
                vote,
            )),
            CollectorInput::WishInput(wish) => CollectorOutput::TimeoutOutput(Self::add_signature(
                &mut self.timeout_collectors,
                &self.notary,
                self.threshold,
                wish.view(),
                wish,
            )),
        }
    }

    fn add_signature<T: Signable + Clone + PartialEq>(
        collectors: &mut BTreeMap<View, CertificateCollector<S, T>>,
        notary: &Notary<S>,
        threshold: u32,
        view: View,
        signature: Signature<S, T>,
    ) -> CertificateOutput<S, T> {
        let collector = collectors
            .entry(view)
            .or_insert_with(|| CertificateCollector::new(threshold));

        if collector.is_ready() {
            return AlreadyReady(view);
        }

        match collector.add_signature(notary, signature) {
            Some(qc) => Ready(qc),
            None => NotReady,
        }
    }
}

struct CertificateCollector<S: Scheme, T: Signable> {
    collected: BTreeMap<NodeID, Signature<S, T>>,
    threshold: u32,
    ready: bool,
}

impl<S: Scheme, T: Signable> CertificateCollector<S, T> {
    fn new(threshold: u32) -> Self {
        Self {
            collected: BTreeMap::new(),
            threshold,
            ready: false,
        }
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn add_signature(
        &mut self,
        notary: &Notary<S>,
        signature: Signature<S, T>,
    ) -> Option<QuorumCertificate<S, T>>
    where
        T: Clone + PartialEq,
    {
        let subject = signature.subject.clone();
        if self.collected.contains_key(&signature.creator()) {
            return None;
        }
        self.collected.insert(signature.creator(), signature);
        let partials: Vec<_> = self
            .collected
            .values()
            .filter(|s| s.subject == subject)
            .map(|s| s.raw.clone())
            .collect();
        if (partials.len() as u32) < self.threshold {
            return None;
        }
        let qc = notary.aggregate(subject, partials.into_iter()).ok()?;
        self.ready = true;
        Some(qc)
    }
}
