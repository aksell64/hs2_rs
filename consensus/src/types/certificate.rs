use codec::{Decode, Encode};
use crate::types::View;
use hotstuff_crypto::sign::{Certificate, Scheme};
use hotstuff_types::threshold::Quorum;

pub type QuorumCertificate<S> = Certificate<<S as Scheme>::Digest, Quorum, <S as Scheme>::Aggregate>;

pub type TimeoutCertificate<S> = Certificate<<S as Scheme>::Digest, Quorum, <S as Scheme>::Aggregate>;

#[derive(Encode, Decode)]
pub struct CertifiedBlock<S: Scheme> {
    pub certificate: QuorumCertificate<S>,
    pub block: S::Digest,
    pub view: View,
}

#[derive(Encode, Decode)]
pub struct CertifiedView<S: Scheme> {
    pub certificate: TimeoutCertificate<S>,
    pub view: View,
}

impl<S: Scheme> Clone for CertifiedBlock<S> {
    fn clone(&self) -> Self {
        Self {
            certificate: self.certificate.clone(),
            block: self.block,
            view: self.view,
        }
    }
}

impl<S: Scheme> Clone for CertifiedView<S> {
    fn clone(&self) -> Self {
        Self {
            certificate: self.certificate.clone(),
            view: self.view,
        }
    }
}
