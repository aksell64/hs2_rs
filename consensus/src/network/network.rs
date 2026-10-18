use crate::network::message::{Envelope, Recipient};
use crate::network::peer::PeerKey;
use crate::types::block::Command;
use bytes::Bytes;
use codec::Encode;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::{NodeID, NodeMap};
use std::fmt::{self, Display};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    UnknownRecipient(NodeID),
    NotConnected(NodeID),
    Backpressure(NodeID),
    TooLarge { size: usize, limit: usize },
    Shutdown,
}

impl Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRecipient(id) => write!(f, "{id:?} is not in the current roster"),
            Self::NotConnected(id) => write!(f, "no live connection to {id:?}"),
            Self::Backpressure(id) => write!(f, "the outbound queue for {id:?} is full"),
            Self::TooLarge { size, limit } => {
                write!(
                    f,
                    "message of {size} bytes exceeds the {limit} byte frame limit"
                )
            }
            Self::Shutdown => f.write_str("the network is shutting down"),
        }
    }
}

impl std::error::Error for SendError {}

pub enum NetworkEvent {
    Message { from: NodeID, bytes: Bytes },
}

pub trait Network: Send + 'static {
    type PeerKey: PeerKey;
    fn update_recipients(&mut self, m: NodeMap<Self::PeerKey>);
    fn send(&self, msg: Bytes, to: NodeID) -> Result<(), SendError>;
    fn broadcast(&self, msg: Bytes) -> Result<(), SendError>;
    fn next(&mut self) -> impl Future<Output = Option<NetworkEvent>> + Send;
}

pub fn send_envelope<S: Scheme, C: Command, N: Network>(
    env: Envelope<S, C>,
    net: &mut N,
) -> Result<(), SendError> {
    match env.to {
        Recipient::All => {
            net.broadcast(env.msg.encode().into())
        },
        Recipient::Node(node_id) => {
            net.send(env.msg.encode().into(), node_id)
        }
    }
}
