use crate::types::block::{Block, Command};
use hotstuff_crypto::sign::Scheme;

pub trait BlockSyncService<S: Scheme, C: Command>: Send + 'static {
    fn request_block(&mut self, hash: S::Digest);

    /// Returns a synced branch ordered from the oldest block to the newest.
    fn next_branch(&mut self) -> impl Future<Output = Vec<Block<S, C>>> + Send;
}
