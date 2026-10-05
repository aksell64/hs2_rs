module network {
  import types.* from "./types"
  import driver.* from "./driver"

  type Packet = {
    sender: NodeID,
    msg: Message,
  }

  // Each inbox is FIFO: packets are delivered in send order, so the model
  // does not explore message reordering. Asynchrony comes only from the
  // interleaving of node steps.
  type Network = {
    inbox: NodeID -> List[Packet],
  }

  pure def send(net: Network, sender: NodeID, receiver: Recipient, msg: Message): Network =
    match receiver {
      | All => net.inbox.keys().fold(net, (acc, node) => deliver(acc, node, { sender: sender, msg: msg }))
      | Node(node) => deliver(net, node, { sender: sender, msg: msg })
    }

  pure def deliver(net: Network, node: NodeID, packet: Packet): Network =
    val packets = if (net.inbox.keys().contains(node)) net.inbox.get(node) else []
    net.with("inbox", net.inbox.put(node, packets.append(packet)))
}
