use crate::network::network::{Network, NetworkEvent, SendError};
use crate::network::peer::PeerKey;
use bytes::Bytes;
use codec::{Decode, Encode};
use hotstuff_types::node::{NodeID, NodeMap};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Encode, Decode)]
pub struct MemoryPeerKey(pub [u8; 4]);

impl AsRef<[u8]> for MemoryPeerKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl PeerKey for MemoryPeerKey {}

pub struct MemoryNetwork {
    id: NodeID,
    peers: Arc<HashMap<NodeID, UnboundedSender<NetworkEvent>>>,
    inbox: UnboundedReceiver<NetworkEvent>,
}

impl MemoryNetwork {
    pub fn connect(ids: impl IntoIterator<Item = NodeID>) -> Vec<MemoryNetwork> {
        let (senders, receivers): (HashMap<_, _>, Vec<_>) = ids
            .into_iter()
            .map(|id| {
                let (tx, rx) = unbounded_channel();
                ((id, tx), (id, rx))
            })
            .unzip();
        let peers = Arc::new(senders);
        receivers
            .into_iter()
            .map(|(id, inbox)| MemoryNetwork {
                id,
                peers: peers.clone(),
                inbox,
            })
            .collect()
    }

    pub fn id(&self) -> NodeID {
        self.id
    }

    fn deliver(&self, to: NodeID, bytes: Bytes) -> Result<(), SendError> {
        let peer = self.peers.get(&to).ok_or(SendError::UnknownRecipient(to))?;
        peer.send(NetworkEvent::Message { from: self.id, bytes })
            .map_err(|_| SendError::NotConnected(to))
    }
}

impl Network for MemoryNetwork {
    type PeerKey = MemoryPeerKey;

    fn update_recipients(&mut self, _: NodeMap<Self::PeerKey>) {}

    fn send(&self, msg: Bytes, to: NodeID) -> Result<(), SendError> {
        self.deliver(to, msg)
    }

    fn broadcast(&self, msg: Bytes) -> Result<(), SendError> {
        for &to in self.peers.keys() {
            let _ = self.deliver(to, msg.clone());
        }
        Ok(())
    }

    fn next(&mut self) -> impl Future<Output = Option<NetworkEvent>> + Send {
        self.inbox.recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_reaches_every_node_including_sender() {
        let mut nets = MemoryNetwork::connect((0..3).map(NodeID::from));
        nets[0].broadcast(Bytes::from_static(b"hi")).unwrap();
        for net in &mut nets {
            let Some(NetworkEvent::Message { from, bytes }) = net.next().await else {
                panic!("no message");
            };
            assert_eq!(from, NodeID::from(0));
            assert_eq!(&bytes[..], b"hi");
        }
    }

    #[tokio::test]
    async fn send_to_unknown_node_fails() {
        let nets = MemoryNetwork::connect((0..2).map(NodeID::from));
        let err = nets[0].send(Bytes::new(), NodeID::from(7)).unwrap_err();
        assert_eq!(err, SendError::UnknownRecipient(NodeID::from(7)));
    }
}
