use codec::{Decode, Encode};
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

pub enum Recipient {
    All,
    Node(NodeID),
}

pub struct Envelope<S: Scheme, C: Command> {
    pub to: Recipient,
    pub msg: Message<S, C>,
}
