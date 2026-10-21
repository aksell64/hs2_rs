use crate::types::clock::Clock;
pub mod certificate;
pub mod block;
pub type View = u64;
pub mod vote;
pub mod mempool;
pub mod clock;
pub mod crypto;
pub mod wish;
pub mod genesis;
pub mod output;

pub struct ViewTimeout<CL: Clock> {
    pub view: View,
    pub deadline: CL,
}

pub struct EpochTimeouts<CL: Clock> {
    pub timeouts: Vec<ViewTimeout<CL>>,
}
