use crate::protocol::driver::DriverInput;
use crate::protocol::pacemaker::Status;
use crate::types::View;
use crate::types::block::{Command, Proposal};
use crate::types::certificate::{BlockQC, SignedVote, SignedWish, TimeoutQC};
use codec::{Decode, Encode};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

#[derive(Encode, Decode)]
pub enum Message<S: Scheme, C: Command> {
    ProposalMsg(Proposal<S, C>),
    VoteMsg(SignedVote<S>),
    WishMsg(SignedWish<S>),
    StatusMsg { qc: BlockQC<S>, view: View },
    TCMsg(TimeoutQC<S>),
    QCMsg(BlockQC<S>),
}

impl<S: Scheme, C: Command> Clone for Message<S, C> {
    fn clone(&self) -> Self {
        match self {
            Self::ProposalMsg(proposal) => Self::ProposalMsg(proposal.clone()),
            Self::VoteMsg(vote) => Self::VoteMsg(vote.clone()),
            Self::WishMsg(wish) => Self::WishMsg(wish.clone()),
            Self::StatusMsg { qc, view } => Self::StatusMsg {
                qc: qc.clone(),
                view: *view,
            },
            Self::TCMsg(tc) => Self::TCMsg(tc.clone()),
            Self::QCMsg(qc) => Self::QCMsg(qc.clone()),
        }
    }
}

impl<S: Scheme, C: Command> Message<S, C> {
    pub fn into_event(self, sender: NodeID) -> DriverInput<S, C> {
        match self {
            Message::ProposalMsg(proposal) => DriverInput::NewProposal { sender, proposal },
            Message::VoteMsg(signed_vote) => DriverInput::NewVote(signed_vote),
            Message::WishMsg(signed_wish) => DriverInput::NewWish(signed_wish),
            Message::StatusMsg { qc, view } => DriverInput::NewStatus(Status { sender, qc, view }),
            Message::TCMsg(tc) => DriverInput::NewTC(tc),
            Message::QCMsg(qc) => DriverInput::NewQC(qc),
        }
    }
}

#[derive(Clone, Copy, Encode, Decode)]
pub enum Recipient {
    All,
    Node(NodeID),
}

#[derive(Encode, Decode)]
pub struct Envelope<S: Scheme, C: Command> {
    pub to: Recipient,
    pub msg: Message<S, C>,
}

impl<S: Scheme, C: Command> Clone for Envelope<S, C> {
    fn clone(&self) -> Self {
        Self {
            to: self.to,
            msg: self.msg.clone(),
        }
    }
}
