use codec::{Decode, Encode};
use core::fmt::Debug;
use derive_more::{From, Into};
use std::collections::HashMap;

#[derive(Eq, PartialEq, Ord, PartialOrd, From, Into, Clone, Debug, Copy, Encode, Decode, Hash)]
pub struct NodeID(pub u32);

#[derive(From, Into)]
pub struct NodeMap<T>(HashMap<NodeID, T>);
