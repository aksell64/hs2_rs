use crate::types::View;
use codec::{Decode, Encode};
use crate::types::crypto::Signable;

#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct Wish {
    pub view: View,
}

impl Signable for Wish {
    const TAG: u8 = 1;
}
