use crate::types::block::{Block, Command};
use hotstuff_crypto::sign::Scheme;

pub trait Output<S: Scheme, C: Command>: Send + 'static {
    fn on_committed(&mut self, block: Block<S, C>);
}
