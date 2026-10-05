use codec::{Decode, Encode};
use std::fmt::Debug;

pub trait PeerKey:
    AsRef<[u8]> + Clone + Ord + core::hash::Hash + Debug + Encode + Decode + Send + Sync + 'static
{
}
