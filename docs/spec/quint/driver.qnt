module driver {
  import types.* from "./types"
  import storage.* from "./storage"
  import consensus.* from "./consensus"
  import pacemaker.* from "./pacemaker"
  import collector.* from "./collector"
  import effects.* from "./effects"
  import keeper.* from "./keeper"

  type DriverInput =
  | NewProposal({ sender: NodeID, proposal: Proposal })
  | NewVote({ sender: NodeID, vote: GenericVote })
  | NewWish({ sender: NodeID, view: View })
  | NewStatus({ sender: NodeID, qc: CertifiedBlock, view: View })
  | NewTC(CertifiedView)
  | NewQC(CertifiedBlock)
  | ViewTimerFired(View)
  | LockTimerFired(View)
  | BlockSynced(Block)

  type Driver = {
    consensus: Consensus,
    pacemaker: Pacemaker,
    collector: Collector,
    keeper: InputKeeper[DriverInput],
  }

  type DriverResult = {
    driver: Driver,
    kv: KVStorage,
    net: List[Envelope],
    timers: List[TimerCommand],
    replay: List[DriverInput],
    sync: List[Hash],
  }

  pure def init_driver(id: NodeID, genesis: Block, genesis_qc: CertifiedBlock): Driver =
    {
      consensus: init_consensus(id, genesis, genesis_qc),
      pacemaker: init_pacemaker(id, genesis_qc),
      collector: { generic_collectors: Map(), timeout_collectors: Map() },
      keeper: empty_keeper,
    }

  pure def apply_driver(d: Driver, kv: KVStorage, input: DriverInput): DriverResult =
    match input {
      | NewProposal(signed) => handle_proposal(d, kv, signed)
      | NewVote(signed) => on_vote(d, kv, signed)
      | NewWish(wish) => on_wish(d, kv, wish)
      | NewStatus(status) => handle_status(d, kv, status)
      | NewTC(tc) => on_timeout_certificate(d, kv, tc)
      | NewQC(qc) => handle_quorum_certificate(d, kv, qc)
      | ViewTimerFired(v) => on_view_timer_fired(d, kv, v)
      | LockTimerFired(v) => on_lock_timer_fired(d, kv, v)
      | BlockSynced(block) => handle_block_synced(d, kv, block)
    }

  pure def driver_no_change(d: Driver, kv: KVStorage): DriverResult =
    { driver: d, kv: kv, net: [], timers: [], replay: [], sync: [] }

  pure def merge_results(first: DriverResult, second: DriverResult): DriverResult =
    {
      driver: second.driver,
      kv: second.kv,
      net: first.net.concat(second.net),
      timers: first.timers.concat(second.timers),
      replay: first.replay.concat(second.replay),
      sync: first.sync.concat(second.sync),
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

  pure def release_view_inputs(res: DriverResult, view: View): DriverResult =
    val released = release_view(res.driver.keeper, view)
    res.with("driver", res.driver.with("keeper", released.keeper))
      .with("replay", res.replay.concat(released.inputs))

  pure def call_pacemaker(d: Driver, input: PacemakerInput): { driver: Driver, effects: List[Effect], out: PacemakerOutput } =
    val r = apply_pacemaker(d.pacemaker, input)
    { driver: d.with("pacemaker", r.pacemaker), effects: r.effects, out: r.out }

  pure def handle_pacemaker_result(
      kv: KVStorage,
      called: { driver: Driver, effects: List[Effect], out: PacemakerOutput }): DriverResult =
    val base = driver_no_change(called.driver, kv)
    val applied = called.effects.foldl(base, add_effect)
    match called.out {
      | ProposeNow(view) =>
        val released = release_view_inputs(applied, view)
        val request = { hash: block_hash(view, released.driver.pacemaker.id), view: view }
        val proposed = on_propose_request(released.driver, released.kv, request)
        merge_results(released, proposed)
      | ViewAdvanced(view) => release_view_inputs(applied, view)
      | NoPacemakerOutput => applied
    }

  pure def check_missing_parent(kv: KVStorage, hash: Hash): Option[Hash] =
    match get_block_by_hash(kv, hash) {
      | None => Some(hash)
      | Some(_) => None
    }

  pure def handle_proposal(d: Driver, kv: KVStorage, signed: { sender: NodeID, proposal: Proposal }): DriverResult =
    val proposal = signed.proposal
    if (signed.sender != d.pacemaker.get_leader_by_view(proposal.view)
        or not(is_valid_qc(proposal.block.justify))
        or proposal.block.parent != proposal.block.justify.block
        or proposal.block.justify.view > proposal.view)
      driver_no_change(d, kv)
    else
      match check_missing_parent(kv, proposal.block.parent) {
        | Some(missing) => buffer_input(d, kv, missing, NewProposal(signed))
        | None =>
          if (proposal.view != proposal.block.justify.view + 1 and proposal.view > d.pacemaker.get_current_view())
            val parked = d.with("keeper", buffer_for_view(d.keeper, proposal.view, NewProposal(signed)))
            on_quorum_certificate(parked, kv, proposal.block.justify)
          else
            on_proposal(d, kv, signed)
      }

  pure def handle_status(d: Driver, kv: KVStorage, status: { sender: NodeID, qc: CertifiedBlock, view: View }): DriverResult =
    if (not(d.pacemaker.is_leader(status.view)) or not(is_valid_qc(status.qc)))
      driver_no_change(d, kv)
    else
      match check_missing_parent(kv, status.qc.block) {
        | Some(missing) => buffer_input(d, kv, missing, NewStatus(status))
        | None =>
          if (status.view > d.pacemaker.get_current_view())
            val parked = d.with("keeper", buffer_for_view(d.keeper, status.view, NewStatus(status)))
            on_quorum_certificate(parked, kv, status.qc)
          else
            on_status(d, kv, status)
      }

  pure def handle_quorum_certificate(d: Driver, kv: KVStorage, qc: CertifiedBlock): DriverResult =
    if (not(is_valid_qc(qc)))
      driver_no_change(d, kv)
    else
      match check_missing_parent(kv, qc.block) {
        | Some(missing) => buffer_input(d, kv, missing, NewQC(qc))
        | None => on_quorum_certificate(d, kv, qc)
      }

  // Blocksync delivers ancestors before descendants. Stored blocks have already passed Update.
  pure def handle_block_synced(d: Driver, kv: KVStorage, block: Block): DriverResult =
    if (not(is_valid_qc(block.justify)) or not(extends_justify(block, kv)))
      driver_no_change(d, kv)
    else
      on_block_synced(d, kv, block)

  pure def on_proposal(d: Driver, kv: KVStorage, signed: { sender: NodeID, proposal: Proposal }): DriverResult =
    val processed = on_quorum_certificate(d, kv, signed.proposal.block.justify)
    val handled = call_proposal_qc_processed(processed.driver, processed.kv, signed.proposal)
    merge_results(processed, handled)

  pure def on_block_synced(d: Driver, kv: KVStorage, block: Block): DriverResult =
    val released = release_block_inputs(driver_no_change(d, kv), block.hash)
    val updated = call_update_locked(released.driver, released.kv, block)
    val processed = on_quorum_certificate(updated.driver, updated.kv, block.justify)
    merge_results(released, processed)

  pure def buffer_input(d: Driver, kv: KVStorage, missing: Hash, input: DriverInput): DriverResult =
    val parked = d.with("keeper", buffer_for_block(d.keeper, missing, input))
    driver_no_change(parked, kv).with("sync", [missing])

  pure def release_block_inputs(res: DriverResult, hash: Hash): DriverResult =
    val released = release_block(res.driver.keeper, hash)
    res.with("driver", res.driver.with("keeper", released.keeper))
      .with("replay", res.replay.concat(released.inputs))

  pure def call_proposal_qc_processed(d: Driver, kv: KVStorage, proposal: Proposal): DriverResult =
    val processed = call_consensus(d, kv, ReceivedProposal(proposal))
    match processed.out {
      | ConfirmVote(vote) =>
        val updated = call_update_locked(processed.driver, kv, proposal.block)
        val result = driver_no_change(updated.driver, updated.kv)
        val voted = add_net_msg(Node(updated.driver.pacemaker.get_leader_by_view(vote.view + 1)), VoteMsg(vote), result)
        release_block_inputs(voted, proposal.block.hash)
      | RejectVote(block) =>
        val updated = call_update_locked(processed.driver, kv, block)
        release_block_inputs(driver_no_change(updated.driver, updated.kv), block.hash)
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
    else if (signed.vote.view > d.pacemaker.get_current_view())
      val parked = d.with("keeper", buffer_for_view(d.keeper, signed.vote.view, NewVote(signed)))
      driver_no_change(parked, kv)
    else
      val r = apply_collector(d.collector, GenericVoteInput(signed))
      val collected = d.with("collector", r.new_collector)
      match r.out {
        | GenericOutput(generic) =>
          match generic {
            | Ready(qc) => driver_no_change(collected, kv).with("replay", [NewQC(qc)])
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
    handle_pacemaker_result(kv, called)

  pure def on_lock_timer_fired(d: Driver, kv: KVStorage, v: View): DriverResult =
    val called = call_pacemaker(d, LockTimerExpired(v))
    handle_pacemaker_result(kv, called)

  pure def on_status(d: Driver, kv: KVStorage, status: { sender: NodeID, qc: CertifiedBlock, view: View }): DriverResult =
    val called = call_pacemaker(d, ReceivedStatus(status))
    handle_pacemaker_result(kv, called)

  pure def on_timeout_certificate(d: Driver, kv: KVStorage, tc: CertifiedView): DriverResult =
    val called = call_pacemaker(d, ReceivedTC(tc))
    handle_pacemaker_result(kv, called)

  pure def on_quorum_certificate(d: Driver, kv: KVStorage, qc: CertifiedBlock): DriverResult =
    val called = call_pacemaker(d, ReceivedQC(qc))
    handle_pacemaker_result(kv, called)

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

}
