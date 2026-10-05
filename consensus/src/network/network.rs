use crate::network::peer::PeerKey;
use hotstuff_types::node::{NodeID, NodeMap};
use std::fmt::{self, Display};
use std::str::Bytes;

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

trait Network {
    type PeerKey: PeerKey;
    fn update_recipients(&mut self, m: NodeMap<Self::PeerKey>);
    fn send(&self, msg: Bytes, to: NodeID) -> Result<(), SendError>;
    fn broadcast(&self, msg: Bytes);
    fn next(&mut self) -> impl Future<Output = Option<(NodeID, Bytes)>> + Send;
}
