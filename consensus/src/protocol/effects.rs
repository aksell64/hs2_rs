use crate::network::message::{Envelope, Message, Recipient};
use crate::timer::service::TimerCommand;
use crate::types::block::Command;
use crate::types::clock::Clock;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

pub enum Effect<S: Scheme, C: Command, CL: Clock> {
    Send(Envelope<S, C>),
    Timer(TimerCommand<CL>),
}

pub fn send_to<S: Scheme, C: Command, CL: Clock>(to: Recipient, msg: Message<S, C>) -> Effect<S, C, CL> {
    Effect::Send(Envelope { to, msg })
}

pub fn send_to_each<S: Scheme, C: Command, CL: Clock>(
    nodes: impl IntoIterator<Item = NodeID>,
    msg: impl Fn() -> Message<S, C>,
) -> Vec<Effect<S, C, CL>> {
    nodes
        .into_iter()
        .map(|node| send_to(Recipient::Node(node), msg()))
        .collect()
}
