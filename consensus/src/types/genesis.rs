use crate::types::block::{Block, Command};
use crate::types::certificate::BlockQC;
use crate::types::crypto::Notary;
use codec::Encode;
use hotstuff_crypto::sign::Scheme;

pub struct GenesisConfig<S: Scheme, C: Command> {
    pub cmd: C,
    pub qc: BlockQC<S>,
    pub tag: String,
}

pub fn genesis_hash<S: Scheme, C: Command>(notary: &Notary<S>, cmd: &C, salt: &[u8]) -> S::Digest {
    notary.hash(&(salt, cmd).encode())
}

pub fn genesis_parent<S: Scheme>(notary: &Notary<S>) -> S::Digest {
    notary.hash(&[])
}

pub fn genesis_block<S: Scheme, C: Command>(
    notary: &Notary<S>,
    cfg: GenesisConfig<S, C>,
) -> Option<Block<S, C>> {
    let salt = cfg.tag.as_bytes();
    let block = Block {
        parent: genesis_parent(notary),
        hash: genesis_hash(notary, &cfg.cmd, cfg.tag.as_bytes()),
        cmd: cfg.cmd,
        justify: cfg.qc,
        height: 0,
    };
    is_valid_genesis(notary, &block, salt).then_some(block)
}

pub fn is_valid_genesis<S: Scheme, C: Command>(
    notary: &Notary<S>,
    block: &Block<S, C>,
    salt: &[u8],
) -> bool {
    block.height == 0
        && block.parent == genesis_parent(notary)
        && block.hash == genesis_hash(notary, &block.cmd, salt)
        && block.justify.view() == 0
        && block.justify.block() == block.hash
        && notary.verify_qc(&block.justify)
}
