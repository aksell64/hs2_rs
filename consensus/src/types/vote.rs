use codec::{Decode, Encode};
use crate::types::View;
use hotstuff_crypto::sign::Scheme;

#[derive(Encode, Decode)]
pub struct Vote<S: Scheme> {
    pub view: View,
    pub block: S::Digest,
}

impl<S: Scheme> Clone for Vote<S> {
    fn clone(&self) -> Self {
        Self {
            view: self.view,
            block: self.block,
        }
    }
}

impl<S: Scheme> PartialEq for Vote<S> {
    fn eq(&self, other: &Self) -> bool {
        self.view == other.view && self.block == other.block
    }
}
