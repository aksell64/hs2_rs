module state_machine {
  import types.* from "./types"
  import storage.* from "./storage"
  import consensus.* from "./consensus"
  import pacemaker.* from "./pacemaker"
  import collector.* from "./collector"
  import driver.* from "./driver"
  import network.* from "./network"
  import effects.* from "./effects"

  type NodeState = {
    node_id: NodeID,
    driver: Driver,
    kv: KVStorage,
    queue: List[DriverInput],
    view_timers: Set[View],
    lock_timer: Option[View],
  }

  type SmOutput =
  | NoSmOutput
  | MissingBlock(Hash)

  type StepResult = {
    state: NodeState,
    net: Network,
    out: SmOutput,
  }

  pure def packet_to_input(packet: Packet): DriverInput =
    match packet.msg {
      | ProposalMsg(proposal) => NewProposal({ sender: packet.sender, proposal: proposal })
      | VoteMsg(vote) => NewVote({ sender: packet.sender, vote: vote })
      | WishMsg(view) => NewWish({ sender: packet.sender, view: view })
      | StatusMsg(status) => NewStatus({ sender: packet.sender, qc: status.qc, view: status.view })
      | TCMsg(tc) => NewTC(tc)
    }

  pure def pop_from_network(state: NodeState, net: Network): { input: Option[DriverInput], net: Network } =
    val inbox = if (net.inbox.keys().contains(state.node_id)) net.inbox.get(state.node_id) else []
    if (inbox.length() == 0)
      { input: None, net: net }
    else
      {
        input: Some(packet_to_input(inbox.head())),
        net: net.with("inbox", net.inbox.put(state.node_id, inbox.tail())),
      }

  pure def push_to_queue(state: NodeState, input: DriverInput): NodeState =
    state.with("queue", state.queue.append(input))

  pure def push_to_queue_front(state: NodeState, input: DriverInput): NodeState =
    state.with("queue", [input].concat(state.queue))

  pure def pop_from_queue(state: NodeState): { input: Option[DriverInput], state: NodeState } =
    if (state.queue.length() == 0)
      { input: None, state: state }
    else
      { input: Some(state.queue.head()), state: state.with("queue", state.queue.tail()) }

  pure def handle_driver_output(state: NodeState, out: DriverOutput): NodeState =
    match out {
      | Replay(input) => push_to_queue_front(state, input)
    }

  pure def apply_timer(state: NodeState, t: TimerCommand): NodeState =
    match t {
      | SetViewTimer(v) => state.with("view_timers", state.view_timers.union(Set(v)))
      | SetLockTimer(v) => state.with("lock_timer", Some(v))
      | ClearLockTimer(v) => if (state.lock_timer == Some(v)) state.with("lock_timer", None) else state
      | ClearViewTimersBelow(v) => state.with("view_timers", state.view_timers.filter(w => w >= v))
    }

  pure def check_missing_parent(kv: KVStorage, input: DriverInput): SmOutput =
    match input {
      | NewProposal(signed) =>
        val parent = signed.proposal.block.parent
        match get_block_by_hash(kv, parent) {
          | Some(_) => NoSmOutput
          | None => MissingBlock(parent)
        }
      | _ => NoSmOutput
    }

  pure def apply_state(state: NodeState, net: Network): StepResult =
    val received = pop_from_network(state, net)
    val queued =
      match received.input {
        | Some(input) => push_to_queue(state, input)
        | None => state
      }
    val popped = pop_from_queue(queued)
    match popped.input {
      | Some(input) =>
        val missing = check_missing_parent(popped.state.kv, input)
        if (missing != NoSmOutput)
          { state: popped.state, net: received.net, out: missing }
        else
          val res = apply_driver(popped.state.driver, popped.state.kv, input)
          val updated = popped.state.with("driver", res.driver).with("kv", res.kv)
          val replayed = res.out.foldl(updated, handle_driver_output)
          val timed = res.timers.foldl(replayed, apply_timer)
          {
            state: timed,
            net: res.net.foldl(received.net, (acc, env) => send(acc, state.node_id, env.to, env.msg)),
            out: NoSmOutput,
          }
      | None => { state: popped.state, net: received.net, out: NoSmOutput }
    }
}
