use codec::{Decode, Encode};
use crate::protocol::driver::DriverInput;
use crate::protocol::pacemaker::Status;
use crate::types::View;
use crate::types::block::{Command, Proposal};
use crate::types::certificate::{BlockQC, TimeoutQC};
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;


#[derive(Encode, Decode)]
pub enum Message<S: Scheme, C: Command> {
    ProposalMsg(Proposal<S, C>),
    VoteMsg(Vote<S>),
    WishMsg(View),
    StatusMsg { qc: BlockQC<S>, view: View },
    TCMsg(TimeoutQC<S>),
}

impl<S: Scheme, C: Command> Message<S, C> {
    pub(crate) fn into_event(self, sender: NodeID) -> DriverInput<S, C> {
        match self {
            Message::ProposalMsg(proposal) => DriverInput::NewProposal { sender, proposal },
            Message::VoteMsg(vote) => DriverInput::NewVote { sender, vote },
            Message::WishMsg(view) => DriverInput::NewWish { sender, view },
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
