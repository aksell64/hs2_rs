use crate::storage::kv::{KVError, ReadStorage, ReadWriteStorage};
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::CertifiedBlock;
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
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

pub struct Consensus<S: Scheme> {
    scheme: Arc<S>,
}

type ConsensusResult<S, C> = Result<Option<ConsensusOutput<S, C>>, KVError>;

impl<S: Scheme> Consensus<S> {
    pub fn new(scheme: Arc<S>) -> Self {
        Self { scheme }
    }

    pub fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
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

    fn on_receive_proposal<C: Command>(&self, proposal: Proposal<S, C>) -> Option<ConsensusOutput<S, C>> {
        if !self.is_valid_qc(&proposal.block.justify) {
            return None;
        }
        Some(ConsensusOutput::ReceiveQC {
            qc: proposal.block.justify.clone(),
            proposal,
        })
    }

    fn on_qc_processed<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        cur_view: View,
        proposal: Proposal<S, C>,
    ) -> ConsensusResult<S, C> {
        let block = proposal.block;
        if proposal.view != cur_view || !Self::extends_justify(&*storage, &block)? {
            return Ok(None);
        }
        if !self.can_vote(storage, &block, cur_view)? {
            return Ok(Some(ConsensusOutput::RejectVote(block)));
        }
        storage.update_voted_view(cur_view)?;
        Ok(Some(ConsensusOutput::ConfirmVote {
            vote: Vote {
                view: cur_view,
                block: block.hash,
            },
            block,
        }))
    }

    fn extends_justify<C: Command, DB: ReadStorage<S, C>>(
        storage: &DB,
        block: &Block<S, C>,
    ) -> Result<bool, KVError> {
        if block.parent != block.justify.block {
            return Ok(false);
        }
        Ok(match storage.get_block_by_hash(&block.parent)? {
            Some(parent) => block.height == parent.height + 1,
            None => false,
        })
    }

    fn can_vote<C: Command, DB: ReadStorage<S, C>>(
        &self,
        storage: &DB,
        block: &Block<S, C>,
        view: View,
    ) -> Result<bool, KVError> {
        let voted_view = storage.get_voted_view()?;
        let locked_qc = storage.get_locked_qc()?;
        if voted_view < view && block.justify.view >= locked_qc.view {
            return Ok(true);
        }
        Ok(false)
    }

    fn update<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        block: Block<S, C>,
    ) -> ConsensusResult<S, C> {
        let locked_qc = storage.get_locked_qc()?;
        if block.justify.view > locked_qc.view {
            storage.update_locked_qc(block.justify.clone())?;
        }
        let Some(justify_block) = storage.get_block_by_hash(&block.justify.block)? else {
            return Ok(None);
        };
        let Some(b) = storage.get_block_by_hash(&justify_block.justify.block)? else {
            return Ok(None);
        };

        let exec_height = storage.get_exec_height()?;
        if justify_block.parent == b.hash
            && block.justify.view == justify_block.justify.view + 1
            && b.height > exec_height
        {
            let exec_hash = storage.get_exec_hash()?;
            self.commit_up_to(storage, b, exec_height, exec_hash)
        } else {
            Ok(None)
        }
    }

    fn commit_up_to<C: Command, DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        target: Block<S, C>,
        exec_height: u64,
        exec_hash: S::Digest,
    ) -> ConsensusResult<S, C> {
        let depth = (target.height - exec_height) as usize;
        let (hash, height) = (target.hash, target.height);
        let mut branch = storage.get_block_branch(target, depth)?;
        if branch.len() != depth || branch[depth - 1].parent != exec_hash {
            return Ok(None);
        }
        storage.update_exec_hash(hash)?;
        storage.update_exec_height(height)?;
        branch.reverse();
        Ok(Some(ConsensusOutput::Commit(branch)))
    }

    fn on_propose<C: Command, DB: ReadStorage<S, C>>(
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
        Ok(Some(ConsensusOutput::Propose(Proposal {
            block,
            view: request.view,
        })))
    }

    fn is_valid_qc(&self, qc: &CertifiedBlock<S>) -> bool {
        self.scheme.verify_certificate(&qc.certificate)
    }
}
