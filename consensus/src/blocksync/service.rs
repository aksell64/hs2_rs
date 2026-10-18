use crate::types::block::{Block, Command};
use hotstuff_crypto::sign::Scheme;

pub trait BlockSyncService<S: Scheme, C: Command>: Send + 'static {
    fn request_block(&mut self, hash: S::Digest);

    fn next_block(&mut self) -> impl Future<Output = Block<S, C>> + Send;
}
