use crate::storage::kv::{KVError, Storage};
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::CertifiedBlock;
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use std::marker::PhantomData;
use std::sync::Arc;

pub struct ProposeRequest<S: Scheme, C: Command> {
    pub hash: S::Digest,
    pub cmd: C,
    pub view: View,
}

pub enum ConsensusEvent<S: Scheme, C: Command> {
    ReceivedProposal(Proposal<S, C>),
    QCProcessed(Proposal<S, C>),
    Update(Block<S, C>),
    MakeProposal(ProposeRequest<S, C>),
}

pub enum ConsensusOutput<S: Scheme, C: Command> {
    ReceiveQC {
        qc: CertifiedBlock<S>,
        proposal: Proposal<S, C>,
    },
    ConfirmVote {
        vote: Vote<S>,
        block: Block<S, C>,
    },
    RejectVote(Block<S, C>),
    Commit(Vec<Block<S, C>>),
    Propose(Proposal<S, C>),
}

pub struct Consensus<S: Scheme, C: Command> {
    scheme: Arc<S>,
    qc_lock: CertifiedBlock<S>,
    voted_view: View,
    exec_hash: S::Digest,
    exec_height: u64,
    _command: PhantomData<C>,
}

type ConsensusResult<S, C> = Result<Option<ConsensusOutput<S, C>>, KVError>;

impl<S: Scheme, C: Command> Consensus<S, C> {
    pub fn new(scheme: Arc<S>, genesis: &Block<S, C>, genesis_qc: CertifiedBlock<S>) -> Self {
        Self {
            scheme,
            qc_lock: genesis_qc,
            voted_view: 0,
            exec_hash: genesis.hash,
            exec_height: genesis.height,
            _command: PhantomData,
        }
    }

    pub fn apply<DB: Storage<S, C>>(
        &mut self,
        storage: &DB,
        cur_view: View,
        high_qc: &CertifiedBlock<S>,
        event: ConsensusEvent<S, C>,
    ) -> ConsensusResult<S, C> {
        match event {
            ConsensusEvent::ReceivedProposal(proposal) => Ok(self.on_receive_proposal(proposal)),
            ConsensusEvent::QCProcessed(proposal) => self.on_qc_processed(storage, cur_view, proposal),
            ConsensusEvent::Update(block) => self.update(storage, block),
            ConsensusEvent::MakeProposal(request) => self.on_propose(storage, high_qc, request),
        }
    }

    fn on_receive_proposal(&self, proposal: Proposal<S, C>) -> Option<ConsensusOutput<S, C>> {
        if !self.is_valid_qc(&proposal.block.justify) {
            return None;
        }
        Some(ConsensusOutput::ReceiveQC {
            qc: proposal.block.justify.clone(),
            proposal,
        })
    }

    fn on_qc_processed<DB: Storage<S, C>>(
        &mut self,
        storage: &DB,
        cur_view: View,
        proposal: Proposal<S, C>,
    ) -> ConsensusResult<S, C> {
        let block = proposal.block;
        if proposal.view != cur_view || !Self::extends_justify(storage, &block)? {
            return Ok(None);
        }
        if !self.can_vote(&block, cur_view) {
            return Ok(Some(ConsensusOutput::RejectVote(block)));
        }
        self.voted_view = cur_view;
        Ok(Some(ConsensusOutput::ConfirmVote {
            vote: Vote { view: cur_view, block: block.hash },
            block,
        }))
    }

    fn extends_justify<DB: Storage<S, C>>(storage: &DB, block: &Block<S, C>) -> Result<bool, KVError> {
        if block.parent != block.justify.block {
            return Ok(false);
        }
        Ok(match storage.get_block_by_hash(&block.parent)? {
            Some(parent) => block.height == parent.height + 1,
            None => false,
        })
    }

    fn can_vote(&self, block: &Block<S, C>, view: View) -> bool {
        self.voted_view < view && block.justify.view >= self.qc_lock.view
    }

    fn update<DB: Storage<S, C>>(&mut self, storage: &DB, b_star: Block<S, C>) -> ConsensusResult<S, C> {
        if b_star.justify.view > self.qc_lock.view {
            self.qc_lock = b_star.justify.clone();
        }
        let Some(b_prime) = storage.get_block_by_hash(&b_star.justify.block)? else {
            return Ok(None);
        };
        let Some(b) = storage.get_block_by_hash(&b_prime.justify.block)? else {
            return Ok(None);
        };
        if b_prime.parent == b.hash
            && b_star.justify.view == b_prime.justify.view + 1
            && b.height > self.exec_height
        {
            self.commit_up_to(storage, b)
        } else {
            Ok(None)
        }
    }

    fn commit_up_to<DB: Storage<S, C>>(&mut self, storage: &DB, target: Block<S, C>) -> ConsensusResult<S, C> {
        let depth = (target.height - self.exec_height) as usize;
        let (hash, height) = (target.hash, target.height);
        let mut branch = storage.get_block_branch(target, depth)?;
        if branch.len() != depth || branch[depth - 1].parent != self.exec_hash {
            return Ok(None);
        }
        self.exec_hash = hash;
        self.exec_height = height;
        branch.reverse();
        Ok(Some(ConsensusOutput::Commit(branch)))
    }

    fn on_propose<DB: Storage<S, C>>(
        &self,
        storage: &DB,
        high_qc: &CertifiedBlock<S>,
        request: ProposeRequest<S, C>,
    ) -> ConsensusResult<S, C> {
        let Some(parent) = storage.get_block_by_hash(&high_qc.block)? else {
            return Ok(None);
        };
        let block = Block {
            hash: request.hash,
            parent: parent.hash,
            cmd: request.cmd,
            justify: high_qc.clone(),
            height: parent.height + 1,
        };
        Ok(Some(ConsensusOutput::Propose(Proposal { block, view: request.view })))
    }

    fn is_valid_qc(&self, qc: &CertifiedBlock<S>) -> bool {
        self.scheme.verify_certificate(&qc.certificate)
    }
}
