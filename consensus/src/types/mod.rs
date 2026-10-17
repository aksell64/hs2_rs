use crate::types::clock::Clock;

mod committee;
pub mod certificate;
pub mod block;
pub type View = u64;
pub mod vote;
pub mod mempool;
pub mod clock;
pub mod crypto;

pub struct ViewTimeout<CL: Clock> {
    pub view: View,
    pub deadline: CL,
}

pub struct EpochTimeouts<CL: Clock> {
    pub timeouts: Vec<ViewTimeout<CL>>,
}
