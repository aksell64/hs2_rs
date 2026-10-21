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
}

impl<S: Scheme, C: Command> Message<S, C> {
    pub fn into_event(self, sender: NodeID) -> DriverInput<S, C> {
        match self {
            Message::ProposalMsg(proposal) => DriverInput::NewProposal { sender, proposal },
            Message::VoteMsg(signed_vote) => DriverInput::NewVote(signed_vote),
            Message::WishMsg(signed_wish) => DriverInput::NewWish(signed_wish),
            Message::StatusMsg { qc, view } => DriverInput::NewStatus(Status { sender, qc, view }),
            Message::TCMsg(tc) => DriverInput::NewTC(tc),
        }
    }
}

pub enum Recipient {
    All,
    Node(NodeID),
}

pub struct Envelope<S: Scheme, C: Command> {
    pub to: Recipient,
    pub msg: Message<S, C>,
}
