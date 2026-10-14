use codec::{Decode, Encode};
use crate::protocol::driver::DriverEvent;
use crate::types::View;
use crate::types::block::{Command, Proposal};
use crate::types::certificate::{CertifiedBlock, CertifiedView};
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;


#[derive(Encode, Decode)]
pub enum Message<S: Scheme, C: Command> {
    ProposalMsg(Proposal<S, C>),
    VoteMsg(Vote<S>),
    WishMsg(View),
    StatusMsg { qc: CertifiedBlock<S>, view: View },
    TCMsg(CertifiedView<S>),
}

impl<S: Scheme, C: Command> Message<S, C> {
    pub(crate) fn into_event(self, sender: NodeID) -> DriverEvent<S, C> {
        match self {
            Message::ProposalMsg(proposal) => DriverEvent::NewProposal { sender, proposal },
            Message::VoteMsg(vote) => DriverEvent::NewVote { sender, vote },
            Message::WishMsg(view) => DriverEvent::NewWish { sender, view },
            Message::StatusMsg { qc, view } => DriverEvent::NewStatus { sender, qc, view },
            Message::TCMsg(tc) => DriverEvent::NewTC(tc),
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
