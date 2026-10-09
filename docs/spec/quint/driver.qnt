module driver {
  import types.* from "./types"
  import storage.* from "./storage"
  import consensus.* from "./consensus"
  import pacemaker.* from "./pacemaker"
  import collector.* from "./collector"

  type DriverInput =
  | NewProposal({ sender: NodeID, proposal: Proposal })
  | NewVote({ sender: NodeID, vote: GenericVote })
  | NewWish({ sender: NodeID, view: View })
  | NewStatus({ sender: NodeID, qc: CertifiedBlock, view: View })
  | NewTC(CertifiedView)
  | ViewTimerFired(View)
  | LockTimerFired(View)
  | EnterView(View)
  | BlockSynced(Block)

  type DriverOutput =
  | NewView(View)
  | SetViewTimer(View)
  | SetLockTimer(View)
  | ClearLockTimer(View)
  | Replay(DriverInput)

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

  type Driver = {
    consensus: Consensus,
    pacemaker: Pacemaker,
    collector: Collector,
    pending: View -> List[{ sender: NodeID, proposal: Proposal }],
  }

  type DriverResult = {
    driver: Driver,
    kv: KVStorage,
    net: List[Envelope],
    out: List[DriverOutput],
  }

  pure def init_driver(id: NodeID, genesis: Block, genesis_qc: CertifiedBlock): Driver =
    {
      consensus: init_consensus(id, genesis, genesis_qc),
      pacemaker: init_pacemaker(id, genesis_qc),
      collector: { generic_collectors: Map(), timeout_collectors: Map() },
      pending: Map(),
    }

  pure def apply_driver(d: Driver, kv: KVStorage, input: DriverInput): DriverResult =
    match input {
      | NewProposal(signed) => on_proposal(d, kv, signed)
      | NewVote(signed) => on_vote(d, kv, signed)
      | NewWish(wish) => on_wish(d, kv, wish)
      | NewStatus(status) => on_status(d, kv, status)
      | NewTC(tc) => on_timeout_certificate(d, kv, tc)
      | ViewTimerFired(v) => on_view_timer_fired(d, kv, v)
      | LockTimerFired(v) => on_lock_timer_fired(d, kv, v)
      | EnterView(v) => call_enter_view(d, kv, v)
      | BlockSynced(_) => driver_no_change(d, kv)
    }

  pure def driver_no_change(d: Driver, kv: KVStorage): DriverResult =
    { driver: d, kv: kv, net: [], out: [] }

  pure def add_out(o: DriverOutput, res: DriverResult): DriverResult =
    res.with("out", res.out.append(o))

  pure def merge_results(first: DriverResult, second: DriverResult): DriverResult =
    {
      driver: second.driver,
      kv: second.kv,
      net: first.net.concat(second.net),
      out: first.out.concat(second.out),
    }

  pure def prepend_outs(outs: List[DriverOutput], res: DriverResult): DriverResult =
    res.with("out", outs.concat(res.out))

  pure def add_net_msg(receiver: Recipient, msg: Message, res: DriverResult): DriverResult =
    res.with("net", res.net.append({ to: receiver, msg: msg }))

  pure def add_net_msg_to_each(nodes: Set[NodeID], msg: Message, res: DriverResult): DriverResult =
    nodes.fold(res, (r, node) => add_net_msg(Node(node), msg, r))

  pure def call_consensus(d: Driver, kv: KVStorage, input: ConsensusInput): { driver: Driver, out: ConsensusOutput } =
    val r = apply_consensus(d.consensus, kv, d.pacemaker.get_current_view(), d.pacemaker.get_high_qc(), input)
    { driver: d.with("consensus", r.consensus), out: r.out }

  pure def call_pacemaker(d: Driver, input: PacemakerInput): { driver: Driver, out: PacemakerOutput } =
    val r = apply_pacemaker(d.pacemaker, input)
    { driver: d.with("pacemaker", r.pacemaker), out: r.out }

  pure def on_proposal(d: Driver, kv: KVStorage, signed: { sender: NodeID, proposal: Proposal }): DriverResult =
    val proposal = signed.proposal
    if (signed.sender != d.pacemaker.get_leader_by_view(proposal.view))
      driver_no_change(d, kv)
    else
      val received = call_consensus(d, kv, ReceivedProposal(proposal))
      match received.out {
        | ReceiveQC(qc) =>
          val pcm = call_pacemaker(received.driver, ProcessQC(qc))
          match pcm.out {
            | HighQCUpdated(high_qc) =>
              val advanced = call_process_new_high_qc(pcm.driver, kv, high_qc)
              merge_results(advanced, call_proposal_or_buffer(advanced.driver, advanced.kv, signed))
            | _ => call_proposal_or_buffer(pcm.driver, kv, signed)
          }
        | _ => driver_no_change(received.driver, kv)
      }

  pure def call_proposal_qc_processed(d: Driver, kv: KVStorage, proposal: Proposal): DriverResult =
    val processed = call_consensus(d, kv, QCProcessed(proposal))
    match processed.out {
      | ConfirmVote(vote) =>
        val updated = call_update_locked(processed.driver, kv, proposal.block)
        val result = driver_no_change(updated.driver, updated.kv)
        add_net_msg(Node(updated.driver.pacemaker.get_leader_by_view(vote.view + 1)), VoteMsg(vote), result)
      | RejectVote(block) =>
        val updated = call_update_locked(processed.driver, kv, block)
        driver_no_change(updated.driver, updated.kv)
      | _ => driver_no_change(processed.driver, kv)
    }

  pure def call_update_locked(d: Driver, kv: KVStorage, block: Block): { driver: Driver, kv: KVStorage } =
    val stored = insert_block(kv, block)
    val updated = call_consensus(d, stored, Update(block))
    match updated.out {
      | Commit(blocks) => { driver: updated.driver, kv: on_commit(stored, blocks) }
      | _ => { driver: updated.driver, kv: stored }
    }

  pure def on_commit(kv: KVStorage, blocks: List[Block]): KVStorage =
    blocks.foldl(kv, (acc, b) => commit_block(acc, b))

  pure def on_vote(d: Driver, kv: KVStorage, signed: { sender: NodeID, vote: GenericVote }): DriverResult =
    if (not(d.pacemaker.is_leader(signed.vote.view + 1)))
      driver_no_change(d, kv)
    else
      val r = apply_collector(d.collector, GenericVoteInput(signed))
      val collected = d.with("collector", r.new_collector)
      match r.out {
        | GenericOutput(generic) =>
          match generic {
            | Ready(qc) =>
              val pcm = call_pacemaker(collected, ProcessQC(qc))
              match pcm.out {
                | HighQCUpdated(high_qc) => call_process_new_high_qc(pcm.driver, kv, high_qc)
                | _ => driver_no_change(pcm.driver, kv)
              }
            | _ => driver_no_change(collected, kv)
          }
        | _ => driver_no_change(collected, kv)
      }

  pure def on_propose_request(d: Driver, kv: KVStorage, request: ProposeRequest): DriverResult =
    val proposed = call_consensus(d, kv, MakeProposal(request))
    match proposed.out {
      | Propose(proposal) =>
        val result = driver_no_change(proposed.driver, kv)
        add_net_msg(All, ProposalMsg(proposal), result)
      | _ => driver_no_change(proposed.driver, kv)
    }

  pure def on_view_timer_fired(d: Driver, kv: KVStorage, v: View): DriverResult =
    val expired = call_pacemaker(d, TimerExpiredView(v))
    match expired.out {
      | AdvanceView(new_view) => call_advance_view(expired.driver, kv, new_view)
      | SendWish(wish) =>
        val result = driver_no_change(expired.driver, kv)
        add_net_msg_to_each(wish.to, WishMsg(wish.view), result)
      | _ => driver_no_change(expired.driver, kv)
    }

  pure def on_lock_timer_fired(d: Driver, kv: KVStorage, v: View): DriverResult =
    val expired = call_pacemaker(d, LockTimerExpired(v))
    match expired.out {
      | ProposeNow(view) =>
        on_propose_request(expired.driver, kv, {
          hash: block_hash(view, expired.driver.pacemaker.id),
          view: view,
        })
      | _ => driver_no_change(expired.driver, kv)
    }

  pure def on_status(d: Driver, kv: KVStorage, status: { sender: NodeID, qc: CertifiedBlock, view: View }): DriverResult =
    val received = call_pacemaker(d, ReceiveStatus(status))
    match received.out {
      | StatusIsLeader(s) =>
        val pcm = call_pacemaker(received.driver, ProcessQC(s.qc))
        match pcm.out {
          | HighQCUpdated(high_qc) => call_process_new_high_qc(pcm.driver, kv, high_qc)
          | _ => driver_no_change(pcm.driver, kv)
        }
      | _ => driver_no_change(received.driver, kv)
    }

  pure def call_advance_view(d: Driver, kv: KVStorage, v: View): DriverResult =
    val advanced = call_pacemaker(d, AdvanceViewRequested(v))
    match advanced.out {
      | CancelLockTimer(timer) =>
        val canceled = call_pacemaker(advanced.driver, LockTimerCanceled(timer.next))
        match canceled.out {
          | StartTimerView(timer_view) =>
            val started = call_pacemaker(canceled.driver, ViewTimerStarted(timer_view))
            val timers = prepend_outs([ClearLockTimer(timer.view), SetViewTimer(timer_view)], driver_no_change(started.driver, kv))
            match started.out {
              | EnterNewView(new_view) => add_out(NewView(new_view), timers)
              | _ => timers
            }
          | _ => add_out(ClearLockTimer(timer.view), driver_no_change(canceled.driver, kv))
        }
      | _ => driver_no_change(advanced.driver, kv)
    }

  pure def on_wish(d: Driver, kv: KVStorage, wish: { sender: NodeID, view: View }): DriverResult =
    val received = call_pacemaker(d, ReceiveWish(wish))
    match received.out {
      | ReceiveWishIsLeader(w) => call_wish_collector(received.driver, kv, w)
      | _ => driver_no_change(received.driver, kv)
    }

  pure def call_wish_collector(d: Driver, kv: KVStorage, wish: { sender: NodeID, view: View }): DriverResult =
    val r = apply_collector(d.collector, WishInput(wish))
    val collected = d.with("collector", r.new_collector)
    match r.out {
      | TimeoutOutput(timeout) =>
        match timeout {
          | TimeoutReady(tc) => on_timeout_certificate(collected, kv, tc)
          | _ => driver_no_change(collected, kv)
        }
      | _ => driver_no_change(collected, kv)
    }

  pure def on_timeout_certificate(d: Driver, kv: KVStorage, tc: CertifiedView): DriverResult =
    val relayed = call_pacemaker(d, TimeoutQC(tc))
    match relayed.out {
      | BroadcastTC(_) =>
        val advanced = call_advance_view(relayed.driver, kv, tc.view)
        add_net_msg(All, TCMsg(tc), advanced)
      | SendTCToLeader(relay) =>
        val advanced = call_advance_view(relayed.driver, kv, tc.view)
        add_net_msg_to_each(relay.to, TCMsg(tc), advanced)
      | _ => driver_no_change(relayed.driver, kv)
    }

  pure def pending_at(d: Driver, v: View): List[{ sender: NodeID, proposal: Proposal }] =
    if (d.pending.keys().contains(v)) d.pending.get(v) else []

  pure def call_proposal_or_buffer(d: Driver, kv: KVStorage, signed: { sender: NodeID, proposal: Proposal }): DriverResult =
    val v = signed.proposal.view
    if (v > d.pacemaker.get_current_view())
      driver_no_change(d.with("pending", d.pending.put(v, pending_at(d, v).append(signed))), kv)
    else
      call_proposal_qc_processed(d, kv, signed.proposal)

  pure def call_enter_view(d: Driver, kv: KVStorage, v: View): DriverResult =
    val released = pending_at(d, v)
    val res = call_enter_view_requested(d.with("pending", d.pending.put(v, [])), kv, v)
    released.foldl(res, (r, signed) => add_out(Replay(NewProposal(signed)), r))

  pure def call_enter_view_requested(d: Driver, kv: KVStorage, v: View): DriverResult =
    val entered = call_pacemaker(d, EnterViewRequested(v))
    match entered.out {
      | ProposeNow(view) =>
        on_propose_request(entered.driver, kv, {
          hash: block_hash(view, entered.driver.pacemaker.id),
          view: view,
        })
      | StartLockTimer(lock_view) => add_out(SetLockTimer(lock_view), driver_no_change(entered.driver, kv))
      | SendStatus(status) =>
        val result = driver_no_change(entered.driver, kv)
        add_net_msg(Node(status.to), StatusMsg({ qc: status.qc, view: status.view }), result)
      | _ => driver_no_change(entered.driver, kv)
    }

  pure def call_process_new_high_qc(d: Driver, kv: KVStorage, high_qc: CertifiedBlock): DriverResult =
    val applied = call_pacemaker(d, HighQCApplied(high_qc))
    match applied.out {
      | AdvanceView(new_view) => call_advance_view(applied.driver, kv, new_view)
      | _ => driver_no_change(applied.driver, kv)
    }
}
