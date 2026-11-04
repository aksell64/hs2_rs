use crate::types::block::{Block, Command};
use crate::types::certificate::{BlockQC, QuorumCertificate, Signature};
use crate::types::vote::Vote;
use codec::Encode;
use hotstuff_crypto::sign::Certificate as RawCertificate;
use hotstuff_crypto::sign::{AssemblyError, PartialSignature, Scheme};
use std::sync::Arc;

pub trait Signable: Encode {
    const TAG: u8;

    fn digest<S: Scheme>(&self, scheme: &S) -> S::Digest {
        let mut bytes = vec![Self::TAG];
        self.encode_to(&mut bytes);
        scheme.hash(&bytes)
    }
}

impl<S: Scheme> Signable for Vote<S> {
    const TAG: u8 = 0;
}

pub struct Notary<S: Scheme> {
    scheme: Arc<S>,
}

impl<S: Scheme> Clone for Notary<S> {
    fn clone(&self) -> Self {
        Self {
            scheme: self.scheme.clone(),
        }
    }
}

impl<S: Scheme> Notary<S> {
    pub fn new(scheme: Arc<S>) -> Self {
        Self { scheme }
    }

    pub fn committee_size(&self) -> u32 {
        self.scheme.committee_size()
    }

    pub fn sign<T: Signable>(&self, subject: T) -> Option<Signature<S, T>> {
        let raw = self.scheme.sign(subject.digest(&*self.scheme))?;
        Some(Signature { subject, raw })
    }

    pub fn verify_qc<T: Signable>(&self, qc: &QuorumCertificate<S, T>) -> bool {
        qc.raw.data_hash() == qc.subject.digest(&*self.scheme)
            && self.scheme.verify_certificate(&qc.raw)
    }

    pub fn verify_partial<T: Signable>(&self, partial: &Signature<S, T>) -> bool {
        self.scheme
            .verify_partial(partial.subject.digest(&*self.scheme), &partial.raw)
    }

    pub fn verify_block_hash<C: Command>(&self, block: &Block<S, C>) -> bool {
        block.hash == self.block_hash(&block.parent, &block.cmd, &block.justify, block.height)
    }

    pub fn aggregate<T: Signable>(
        &self,
        subject: T,
        partials: impl Iterator<Item = PartialSignature<S::Signature>>,
    ) -> Result<QuorumCertificate<S, T>, AssemblyError> {
        let data_hash = subject.digest(&*self.scheme);
        let aggregate = self.scheme.aggregate(data_hash, partials)?;
        Ok(QuorumCertificate {
            subject,
            raw: RawCertificate::new(data_hash, aggregate),
        })
    }

    pub fn block_hash<C: Command>(
        &self,
        parent: &S::Digest,
        cmd: &C,
        justify: &BlockQC<S>,
        height: u64,
    ) -> S::Digest {
        self.hash(&(parent, cmd, justify, height).encode())
    }

    pub fn hash(&self, bytes: &[u8]) -> S::Digest {
        self.scheme.hash(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mocks::scheme::{TestScheme, digest, qc};

    fn notary() -> Notary<TestScheme> {
        Notary::new(Arc::new(TestScheme { n: 4, id: None }))
    }

    #[test]
    fn verify_qc_accepts_a_genuine_qc() {
        let notary = notary();
        assert!(notary.verify_qc(&qc(&notary.scheme, 3, digest(7))));
    }

    #[test]
    fn verify_qc_rejects_a_relabeled_view() {
        let notary = notary();
        let mut relabeled = qc(&notary.scheme, 3, digest(7));
        relabeled.subject.view = 10;
        assert!(!notary.verify_qc(&relabeled));
    }

    #[test]
    fn verify_qc_rejects_a_relabeled_block() {
        let notary = notary();
        let mut relabeled = qc(&notary.scheme, 3, digest(7));
        relabeled.subject.block = digest(8);
        assert!(!notary.verify_qc(&relabeled));
    }
}
