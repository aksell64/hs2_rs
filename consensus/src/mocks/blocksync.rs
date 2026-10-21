use crate::blocksync::service::BlockSyncService;
use crate::types::block::{Block, Command};
use hotstuff_crypto::sign::Scheme;

pub struct NoBlockSync;

impl<S: Scheme, C: Command> BlockSyncService<S, C> for NoBlockSync {
    fn request_block(&mut self, _: S::Digest) {}

    fn next_branch(&mut self) -> impl Future<Output = Vec<Block<S, C>>> + Send {
        std::future::pending()
    }
}
