use codec::{Decode, Encode};
use crate::types::View;
use hotstuff_crypto::sign::Scheme;

#[derive(Encode, Decode)]
pub struct Vote<S: Scheme> {
    pub view: View,
    pub block: S::Digest,
}
