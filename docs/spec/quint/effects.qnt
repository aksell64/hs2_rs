module effects {
  import types.* from "./types"

  type Message =
  | ProposalMsg(Proposal)
  | VoteMsg(GenericVote)
  | WishMsg(View)
  | StatusMsg({ qc: CertifiedBlock, view: View })
  | TCMsg(CertifiedView)

  type Recipient =
  | All
  | Node(NodeID)

  type Envelope = {
    to: Recipient,
    msg: Message,
  }

  type TimerCommand =
  | SetViewTimer(View)
  | SetLockTimer(View)
  | ClearLockTimer(View)

  type Effect =
  | Send(Envelope)
  | Timer(TimerCommand)

  pure def send_to(receiver: Recipient, msg: Message): Effect = Send({ to: receiver, msg: msg })

  pure def send_to_each(nodes: Set[NodeID], msg: Message): List[Effect] =
    nodes.fold([], (acc, node) => acc.append(send_to(Node(node), msg)))
}
