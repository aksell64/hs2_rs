module driver {
  import types.* from "./types"
  import storage.* from "./storage"
  import consensus.* from "./consensus"
  import pacemaker.* from "./pacemaker"
  import collector.* from "./collector"
  import effects.* from "./effects"

  type DriverInput =
  | NewProposal({ sender: NodeID, proposal: Proposal })
  | NewVote({ sender: NodeID, vote: GenericVote })
  | NewWish({ sender: NodeID, view: View })
  | NewStatus({ sender: NodeID, qc: CertifiedBlock, view: View })
  | NewTC(CertifiedView)
  | ViewTimerFired(View)
  | LockTimerFired(View)
  | BlockSynced(Block)

  type DriverOutput =
  | Replay(DriverInput)

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
    timers: List[TimerCommand],
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
      | BlockSynced(b) => on_block_synced(d, kv, b)
    }

  pure def driver_no_change(d: Driver, kv: KVStorage): DriverResult =
    { driver: d, kv: kv, net: [], timers: [], out: [] }

  // A block delivered by blocksync: take its QC, store it, advance the lock and commit.
  // No vote: the view of a synced block is in the past.
  pure def on_block_synced(d: Driver, kv: KVStorage, b: Block): DriverResult =
    if (get_block_by_hash(kv, b.hash) != None or get_block_by_hash(kv, b.parent) == None)
      driver_no_change(d, kv)
    else
      val called = call_pacemaker(d, ReceivedQC(b.justify))
      val processed = handle_pacemaker_result(d, kv, called)
      val updated = call_update_locked(processed.driver, processed.kv, b)
      processed.with("driver", updated.driver).with("kv", updated.kv)

  pure def add_out(o: DriverOutput, res: DriverResult): DriverResult =
    res.with("out", res.out.append(o))

  pure def merge_results(first: DriverResult, second: DriverResult): DriverResult =
    {
      driver: second.driver,
      kv: second.kv,
      net: first.net.concat(second.net),
      timers: first.timers.concat(second.timers),
      out: first.out.concat(second.out),
    }

  pure def add_net_msg(receiver: Recipient, msg: Message, res: DriverResult): DriverResult =
    res.with("net", res.net.append({ to: receiver, msg: msg }))

  pure def call_consensus(d: Driver, kv: KVStorage, input: ConsensusInput): { driver: Driver, out: ConsensusOutput } =
    val r = apply_consensus(d.consensus, kv, d.pacemaker.get_current_view(), d.pacemaker.get_high_qc(), input)
    { driver: d.with("consensus", r.consensus), out: r.out }

  pure def add_effect(res: DriverResult, e: Effect): DriverResult =
    match e {
      | Send(env) => add_net_msg(env.to, env.msg, res)
      | Timer(t) => res.with("timers", res.timers.append(t))
    }

  pure def release_pending(res: DriverResult, v: View): DriverResult =
    val released = pending_at(res.driver, v)
    val pending = res.driver.pending.put(v, [])
    val cleared = res.with("driver", res.driver.with("pending", pending))
    released.foldl(cleared, (r, signed) => add_out(Replay(NewProposal(signed)), r))

  pure def call_pacemaker(d: Driver, input: PacemakerInput): { driver: Driver, effects: List[Effect], out: PacemakerOutput } =
    val r = apply_pacemaker(d.pacemaker, input)
    { driver: d.with("pacemaker", r.pacemaker), effects: r.effects, out: r.out }

  pure def handle_pacemaker_result(
      before: Driver,
      kv: KVStorage,
      called: { driver: Driver, effects: List[Effect], out: PacemakerOutput }): DriverResult =
    val old_view = before.pacemaker.get_current_view()
    val base = driver_no_change(called.driver, kv)
    val applied = called.effects.foldl(base, add_effect)
    val new_view = called.driver.pacemaker.get_current_view()
    val released = if (new_view > old_view) release_pending(applied, new_view) else applied
    match called.out {
      | ProposeNow(view) =>
        val request = { hash: block_hash(view, released.driver.pacemaker.id), view: view }
        val proposed = on_propose_request(released.driver, released.kv, request)
        merge_results(released, proposed)
      | NoPacemakerOutput => released
    }

  pure def on_proposal(d: Driver, kv: KVStorage, signed: { sender: NodeID, proposal: Proposal }): DriverResult =
    val proposal = signed.proposal
    if (signed.sender != d.pacemaker.get_leader_by_view(proposal.view)
        or not(is_valid_qc(proposal.block.justify)))
      driver_no_change(d, kv)
    else
      val called = call_pacemaker(d, ReceivedQC(proposal.block.justify))
      val processed = handle_pacemaker_result(d, kv, called)
      val handled = call_proposal_or_buffer(processed.driver, processed.kv, signed)
      merge_results(processed, handled)

  pure def call_proposal_qc_processed(d: Driver, kv: KVStorage, proposal: Proposal): DriverResult =
    val processed = call_consensus(d, kv, ReceivedProposal(proposal))
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
              val called = call_pacemaker(collected, ReceivedQC(qc))
              handle_pacemaker_result(collected, kv, called)
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
    val called = call_pacemaker(d, ViewTimerExpired(v))
    handle_pacemaker_result(d, kv, called)

  pure def on_lock_timer_fired(d: Driver, kv: KVStorage, v: View): DriverResult =
    val called = call_pacemaker(d, LockTimerExpired(v))
    handle_pacemaker_result(d, kv, called)

  pure def on_status(d: Driver, kv: KVStorage, status: { sender: NodeID, qc: CertifiedBlock, view: View }): DriverResult =
    val called = call_pacemaker(d, ReceivedStatus(status))
    handle_pacemaker_result(d, kv, called)

  pure def on_timeout_certificate(d: Driver, kv: KVStorage, tc: CertifiedView): DriverResult =
    val called = call_pacemaker(d, ReceivedTC(tc))
    handle_pacemaker_result(d, kv, called)

  pure def on_wish(d: Driver, kv: KVStorage, wish: { sender: NodeID, view: View }): DriverResult =
    if (epoch_leaders(wish.view).contains(d.pacemaker.id))
      call_wish_collector(d, kv, wish)
    else
      driver_no_change(d, kv)

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

  pure def pending_at(d: Driver, v: View): List[{ sender: NodeID, proposal: Proposal }] =
    if (d.pending.keys().contains(v)) d.pending.get(v) else []

  pure def call_proposal_or_buffer(d: Driver, kv: KVStorage, signed: { sender: NodeID, proposal: Proposal }): DriverResult =
    val v = signed.proposal.view
    if (v > d.pacemaker.get_current_view())
      driver_no_change(d.with("pending", d.pending.put(v, pending_at(d, v).append(signed))), kv)
    else
      call_proposal_qc_processed(d, kv, signed.proposal)

}
