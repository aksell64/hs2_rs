use codec::{Decode, Encode};
use core::fmt::Debug;
use std::collections::{HashMap, HashSet};
use derive_more::{From, Into};

#[derive(Eq, PartialEq, Ord, PartialOrd, From, Into, Clone, Debug, Copy, Encode, Decode, Hash)]
pub struct NodeID(pub u32);


pub struct NodeMap<T>(HashMap<NodeID, T>);

