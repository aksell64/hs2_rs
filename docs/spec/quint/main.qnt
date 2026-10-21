module main {
  import types.* from "./types"
  import storage.* from "./storage"
  import driver.* from "./driver"
  import effects.* from "./effects"
  import network.* from "./network"
  import pacemaker.* from "./pacemaker"
  import state_machine.* from "./sm"
  import invariants.* from "./invariants"
  import blocksync.* from "./blocksync"

  pure val NODES: Set[NodeID] = 0.to(committee_size - 1)
  const FAULTY: Set[NodeID]

  // Mutants only (tools/mutants.py): nodes that run the protocol with a Byzantine strategy
  // applied to their code and are excluded from the invariants. Set() in every model;
  // a strategy run sets it together with FAULTY = Set().
  const BYZANTINE: Set[NodeID]

  pure val RUNNING: Set[NodeID] = NODES.exclude(FAULTY)
  pure val CORRECT: Set[NodeID] = RUNNING.exclude(BYZANTINE)
  const MAX_VIEW: View
  const FAULTY_PERCENT: int
  const TIMER_PERCENT: int
  pure val VIEWS: Set[View] = 0.to(MAX_VIEW)
  pure val VARIANTS: Set[int] = 0.to(hash_variants - 1)
  pure val HASHES: Set[Hash] =
    tuples(VIEWS, NODES, VARIANTS).map(t => block_variant_hash(t._1, t._2, t._3))

  pure val GENESIS_QC: CertifiedBlock = {
    certificate: { signature: QuorumCert(NODES) },
    block: 0,
    view: 0,
  }

  pure val GENESIS: Block = { hash: 0, parent: -1, height: 0, justify: GENESIS_QC }

  var nodes: NodeID -> NodeState
  var prev_nodes: NodeID -> NodeState
  var net: Network
  var qc_views: QCViewCollector
  var all_qcs: Set[CertifiedBlock]
  var syncs: NodeID -> BlockSync
  var sync_net: Set[SyncPacket]
  // Ghost: every block ever created (proposed by a correct leader or sent by a faulty one).
  // A block outside this set is a forgery; checking membership stands for hash integrity.
  var minted: Set[Block]

  pure def init_node(id: NodeID): NodeState =
    {
      node_id: id,
      driver: init_driver(id, GENESIS, GENESIS_QC),
      kv: insert_block(empty_storage, GENESIS),
      queue: [ViewTimerFired(0)],
      view_timers: Set(),
      lock_timer: None,
    }

  action init = all {
    nodes' = RUNNING.mapBy(id => init_node(id)),
    prev_nodes' = RUNNING.mapBy(id => init_node(id)),
    net' = { inbox: RUNNING.mapBy(_ => []) },
    qc_views' = record_qc(Map(), GENESIS_QC),
    all_qcs' = Set(GENESIS_QC),
    syncs' = RUNNING.mapBy(_ => init_blocksync),
    sync_net' = Set(),
    minted' = Set(GENESIS),
  }

  pure def proposed_blocks(n: Network): Set[Block] =
    n.inbox.keys().fold(Set(), (acc, id) => n.inbox.get(id).foldl(acc, (s, packet) => match packet.msg {
      | ProposalMsg(proposal) => s.union(Set(proposal.block))
      | _ => s
    }))

  pure val no_client_result: ClientResult = { blocksync: init_blocksync, packets: Set(), inputs: [] }

  action apply_step(id: NodeID): bool =
    val node = nodes.get(id)
    val r = apply_state(node, net)
    val known = known_hashes(r.state.kv, r.state.queue)
    val missing = match r.out {
      | MissingBlock(m) => on_missing(syncs.get(id), id, known, m.qc, m.parked)
      | NoSmOutput => no_client_result.with("blocksync", syncs.get(id))
    }
    val tidied = tidy(missing.blocksync, known, r.state.kv.block_by_hash.keys())
    val state = r.state.with("queue", tidied.inputs.concat(r.state.queue))
    all {
      nodes' = nodes.put(id, state),
      prev_nodes' = nodes,
      net' = r.net,
      qc_views' = record_qcs_of(qc_views, state),
      all_qcs' = all_qcs.union(qcs_of(state)),
      syncs' = syncs.put(id, tidied.blocksync),
      sync_net' = sync_net.union(missing.packets),
      minted' = minted.union(proposed_blocks(r.net)),
    }

  // Applies a client result of node `id`: inputs to the front of its queue, packets to the network.
  action apply_client(id: NodeID, res: ClientResult, consumed: Set[SyncPacket]): bool =
    val node = nodes.get(id)
    all {
      nodes' = nodes.put(id, node.with("queue", res.inputs.concat(node.queue))),
      prev_nodes' = nodes,
      net' = net,
      qc_views' = qc_views,
      all_qcs' = all_qcs,
      syncs' = syncs.put(id, res.blocksync),
      sync_net' = sync_net.exclude(consumed).union(res.packets),
      minted' = minted,
    }

  def requests_to(id: NodeID): Set[SyncPacket] = sync_net.filter(p => p.to == id and is_request(p))

  def responses_to(id: NodeID): Set[SyncPacket] = sync_net.filter(p => p.to == id and is_response(p))

  // The server of `id` answers one request from its storage.
  action sync_serve(id: NodeID): bool = all {
    requests_to(id).size() > 0,
    nondet p = requests_to(id).oneOf()
    all {
      nodes' = nodes,
      prev_nodes' = nodes,
      net' = net,
      qc_views' = qc_views,
      all_qcs' = all_qcs,
      syncs' = syncs,
      sync_net' = sync_net.exclude(Set(p)).union(serve(nodes.get(id).kv, id, p)),
      minted' = minted,
    },
  }

  // The client of `id` handles one response.
  action sync_receive(id: NodeID): bool = all {
    responses_to(id).size() > 0,
    nondet p = responses_to(id).oneOf()
    val node = nodes.get(id)
    val known = known_hashes(node.kv, node.queue)
    apply_client(id, on_response(syncs.get(id), id, known, node.kv.block_by_hash.keys(), minted, p), Set(p)),
  }

  def in_flight(id: NodeID, hash: Hash): bool =
    val f = syncs.get(id).fetches.get(hash)
    sync_net.exists(p => and {
      packet_hash(p) == hash,
      packet_attempt(p) == f.attempt,
      or {
        p.from == id and p.to == f.peer and not(FAULTY.contains(f.peer)),
        p.from == f.peer and p.to == id,
      },
    })

  // A fetch of `id` times out and goes to the next signer.
  action sync_timeout(id: NodeID, fair: bool): bool =
    val ready = syncs.get(id).fetches.keys().filter(h => not(fair) or not(in_flight(id, h)))
    all {
      ready.size() > 0,
      nondet hash = ready.oneOf()
      apply_client(id, on_timeout(syncs.get(id), id, hash), Set()),
    }

  def sync_timeout_ready_fair(id: NodeID): bool =
    syncs.get(id).fetches.keys().exists(h => not(in_flight(id, h)))

  // Responses a faulty node can send for `hash`: nothing useful, a genuine (possibly truncated)
  // chain, an unrelated block, or a forgery: the requested hash on top of another valid QC.
  def faulty_responses(hash: Hash): Set[List[Block]] =
    val blocks = all_blocks(nodes)
    val kv = empty_storage.with("block_by_hash", blocks)
    val genuine =
      if (blocks.keys().contains(hash))
        1.to(MAX_BLOCKS).map(k => get_block_branch(blocks.get(hash), kv, k))
      else
        Set()
    val forged = all_qcs
      .filter(q => blocks.keys().contains(q.block) and not(blocks.keys().contains(hash) and blocks.get(hash).parent == q.block))
      .map(q => [{ hash: hash, parent: q.block, height: blocks.get(q.block).height + 1, justify: q }])
    val unrelated = blocks.keys().exclude(Set(hash)).map(h => [blocks.get(h)])
    Set([]).union(genuine).union(forged).union(unrelated)

  // A faulty node answers any open request: its own (consuming it) or someone else's (unsolicited).
  action deliver_faulty_sync: bool = all {
    FAULTY.size() > 0,
    sync_net.exists(p => is_request(p)),
    nondet p = sync_net.filter(p => is_request(p)).oneOf()
    nondet from = FAULTY.oneOf()
    nondet blocks = faulty_responses(packet_hash(p)).oneOf()
    val resp = { from: from, to: p.from, msg: BlockResponse({ hash: packet_hash(p), attempt: packet_attempt(p), blocks: blocks }) }
    all {
      nodes' = nodes,
      prev_nodes' = nodes,
      net' = net,
      qc_views' = qc_views,
      all_qcs' = all_qcs,
      syncs' = syncs,
      sync_net' = (if (p.to == from) sync_net.exclude(Set(p)) else sync_net).union(Set(resp)),
      minted' = minted,
    },
  }

  action sync_step(id: NodeID, fair: bool): bool = any {
    sync_serve(id),
    sync_receive(id),
    sync_timeout(id, fair),
  }

  action enqueue_timer(id: NodeID, node: NodeState, input: DriverInput): bool = all {
    nodes' = nodes.put(id, push_to_queue(node, input)),
    prev_nodes' = nodes,
    net' = net,
    qc_views' = qc_views,
    all_qcs' = all_qcs,
    syncs' = syncs,
    sync_net' = sync_net,
    minted' = minted,
  }

  pure def armed_view(timer: Option[View]): View =
    match timer {
      | Some(v) => v
      | None => -1
    }

  pure def next_view_timer(timers: Set[View]): View =
    timers.fold(-1, (m, w) => if (m == -1 or w < m) w else m)

  // The lock timer (P_pc + Δ) is shorter than the view slot (τ), so a leader waiting
  // in Case 2 always gets to propose before its view times out. Without clocks this
  // order is lost; the guard below restores it.
  action fire_view_timer(id: NodeID): bool =
    val node = nodes.get(id)
    val v = next_view_timer(node.view_timers)
    all {
      node.view_timers.size() > 0,
      node.lock_timer == None,
      enqueue_timer(id, node.with("view_timers", node.view_timers.exclude(Set(v))), ViewTimerFired(v)),
    }

  action fire_lock_timer(id: NodeID): bool =
    val node = nodes.get(id)
    all {
      node.lock_timer != None,
      enqueue_timer(id, node.with("lock_timer", None), LockTimerFired(armed_view(node.lock_timer))),
    }

  pure def faulty_votes: Set[Packet] =
    tuples(FAULTY, VIEWS, HASHES).map(t => { sender: t._1, msg: VoteMsg({ view: t._2, block: t._3 }) })

  pure def faulty_proposals(qcs: Set[CertifiedBlock], blocks: Hash -> Block): Set[Packet] =
    tuples(FAULTY, VIEWS, qcs, VARIANTS)
      .filter(t => leader(t._2) == t._1 and blocks.keys().contains(t._3.block))
      .map(t => {
        sender: t._1,
        msg: ProposalMsg({
          block: {
            hash: block_variant_hash(t._2, t._1, t._4),
            parent: t._3.block,
            height: blocks.get(t._3.block).height + 1,
            justify: t._3,
          },
          view: t._2,
        }),
      })

  action deliver_faulty(id: NodeID): bool = {
    val packets = faulty_votes.union(faulty_proposals(all_qcs, all_blocks(nodes)))
    nondet packet = packets.oneOf()
    all {
      nodes' = nodes.put(id, push_to_queue(nodes.get(id), packet_to_input(packet))),
      prev_nodes' = nodes,
      net' = net,
      qc_views' = qc_views,
      all_qcs' = all_qcs,
      syncs' = syncs,
      sync_net' = sync_net,
      minted' = minted.union(match packet.msg { | ProposalMsg(proposal) => Set(proposal.block) | _ => Set() }),
    }
  }

  def has_messages(id: NodeID): bool =
    nodes.get(id).queue.length() > 0 or net.inbox.get(id).length() > 0

  def has_sync_work(id: NodeID): bool =
    requests_to(id).size() > 0 or responses_to(id).size() > 0

  def has_armed_timer(id: NodeID): bool =
    nodes.get(id).view_timers.size() > 0 or nodes.get(id).lock_timer != None

  def actors(dice: int): Set[NodeID] =
    RUNNING.filter(id => or {
      has_messages(id),
      has_sync_work(id),
      dice < TIMER_PERCENT and has_armed_timer(id),
      dice < TIMER_PERCENT and syncs.get(id).fetches.keys().size() > 0,
      dice < FAULTY_PERCENT,
    })

  action step = {
    nondet dice = 0.to(99).oneOf()
    val candidates = actors(dice)
    nondet id = (if (candidates.size() > 0) candidates else RUNNING).oneOf()
    any {
      all { has_messages(id) or candidates.size() == 0, apply_step(id) },
      all { dice < FAULTY_PERCENT, deliver_faulty(id) },
      all { dice < TIMER_PERCENT, fire_view_timer(id) },
      all { dice < TIMER_PERCENT, fire_lock_timer(id) },
      all { dice < FAULTY_PERCENT, deliver_faulty_sync },
      sync_serve(id),
      sync_receive(id),
      all { dice < TIMER_PERCENT, sync_timeout(id, false) },
    }
  }

  def leader_finished(v: View): bool =
    val l = leader(v)
    if (FAULTY.contains(l))
      true
    else
      val pcm = nodes.get(l).driver.pacemaker
      pcm.cur_view > v or (pcm.cur_view == v and pcm.proposed)

  // Fairness assumption on the scheduler: a view timer is only allowed to fire once
  // the useful work of the view is done.
  //
  // Guarantee: a correct node times out of view v only after the leader of v has proposed
  // (or is faulty, or has left v) and the node has processed that proposal, i.e. every
  // correct node gets a chance to vote in every view with a correct leader.
  //
  // Not guaranteed: the node may still time out after voting in v but before the next
  // leader has sent the proposal of v+1 carrying QC(v); then it enters v+1 by the timer
  // instead of by the certificate. Nothing bounds how far apart the views of different
  // nodes can drift, and there is no asynchronous period before this mode starts.
  def view_timer_ready_fair(id: NodeID): bool =
    val node = nodes.get(id)
    and {
      // The node is in some view v and its slot for v is running.
      node.view_timers.size() > 0,
      // No Case 2 wait is in progress: a leader waiting for statuses proposes before its
      // view timer may fire (stands for the lock timer P_pc + Δ being shorter than τ).
      node.lock_timer == None,
      // Everything already sent to the node (in the inbox) or produced by it (in the queue)
      // has been processed (stands for messages being delivered well within τ).
      not(has_messages(id)),
      // The leader of v has done its part: it proposed in v or already left v. Proposing and
      // sending happen in one step, so a proposal of a correct leader is by now in this
      // node's inbox, queue, pending buffer or already processed. A faulty leader counts as
      // done: correct nodes must be able to time out on it.
      leader_finished(next_view_timer(node.view_timers)),
    }

  def actors_fair(dice: int): Set[NodeID] =
    RUNNING.filter(id => or {
      has_messages(id),
      has_sync_work(id),
      sync_timeout_ready_fair(id),
      view_timer_ready_fair(id),
      nodes.get(id).lock_timer != None,
      dice < FAULTY_PERCENT,
    })

  action step_fair = {
    nondet dice = 0.to(99).oneOf()
    val candidates = actors_fair(dice)
    nondet id = (if (candidates.size() > 0) candidates else RUNNING).oneOf()
    any {
      all { has_messages(id) or candidates.size() == 0, apply_step(id) },
      all { dice < FAULTY_PERCENT, deliver_faulty(id) },
      all { view_timer_ready_fair(id), fire_view_timer(id) },
      fire_lock_timer(id),
      all { dice < FAULTY_PERCENT, deliver_faulty_sync },
      sync_serve(id),
      sync_receive(id),
      sync_timeout(id, true),
    }
  }

  action step_happy = {
    val candidates = RUNNING.filter(id => has_messages(id) or has_sync_work(id))
    nondet id = (if (candidates.size() > 0) candidates else RUNNING).oneOf()
    any {
      apply_step(id),
      sync_step(id, true),
    }
  }

  val agreement_inv = agreement(CORRECT.mapBy(id => nodes.get(id)))

  val lock_monotonic_inv = CORRECT.forall(id => monotony_of_lock(nodes.get(id), prev_nodes.get(id)))

  val voted_view_monotonic_inv = CORRECT.forall(id => monotony_of_voted_view(nodes.get(id), prev_nodes.get(id)))

  val high_qc_monotonic_inv = CORRECT.forall(id => monotony_of_high_qc(nodes.get(id), prev_nodes.get(id)))

  val exec_monotonic_inv = CORRECT.forall(id => monotony_of_executed_blocks(nodes.get(id), prev_nodes.get(id)))

  val once_qc_in_view_inv = once_qc_in_view(qc_views)

  val lock_protection_inv = lock_protection(qc_views, all_blocks(nodes))

  val ancestry_inv = certified_extends_justify(qc_views, all_blocks(nodes))

  pure def stored_ancestry(state: NodeState): bool =
    val blocks = state.kv.block_by_hash
    blocks.keys().forall(h => h == 0 or blocks.keys().contains(blocks.get(h).parent))

  val stored_ancestry_inv = CORRECT.forall(id => stored_ancestry(nodes.get(id)))

  val stored_genuine_inv = CORRECT.forall(id =>
    val blocks = nodes.get(id).kv.block_by_hash
    blocks.keys().forall(h => minted.contains(blocks.get(h))))

  val request_gate_inv = CORRECT.forall(id =>
    val fetches = syncs.get(id).fetches
    fetches.keys().forall(h => and {
      is_valid_qc(fetches.get(h).qc),
      fetches.get(h).qc.block == h,
      signers(fetches.get(h).qc).contains(fetches.get(h).peer),
      fetches.get(h).peer != id,
    }))

  // Blocksync only delivers certified blocks: every queued BlockSynced has a QC somewhere.
  val synced_certified_inv =
    val certified = all_qcs.map(q => q.block).union(minted.map(b => b.justify.block))
    CORRECT.forall(id => nodes.get(id).queue.foldl(true, (ok, input) => ok and match input {
      | BlockSynced(b) => certified.contains(b.hash)
      | _ => true
    }))

  val sync_inv = and {
    stored_ancestry_inv,
    stored_genuine_inv,
    request_gate_inv,
    synced_certified_inv,
  }

  val safety_inv = and {
    agreement_inv,
    lock_monotonic_inv,
    voted_view_monotonic_inv,
    high_qc_monotonic_inv,
    exec_monotonic_inv,
    once_qc_in_view_inv,
    lock_protection_inv,
    ancestry_inv,
    sync_inv,
  }

  // Blocksync witnesses (quint run --witnesses): a block was delivered, a fetch was retried,
  // a response was rejected by verification.
  val sync_delivery_witness = CORRECT.exists(id => syncs.get(id).delivered > 0)

  val sync_retry_witness = CORRECT.exists(id => syncs.get(id).retries > 0)

  val sync_reject_witness = CORRECT.exists(id => syncs.get(id).rejected > 0)

  val no_commit_witness = CORRECT.forall(id => nodes.get(id).kv.committed_branch.length() == 0)

  // Tail-forking: the block certified in view v is dropped by the certified block of view v + 1.
  val tail_fork_witness =
    val blocks = all_blocks(nodes)
    qc_views.keys().exists(v => qc_views.keys().contains(v + 1) and
      qc_views.get(v).exists(b => qc_views.get(v + 1).exists(c =>
        blocks.keys().contains(c) and blocks.get(c).parent != b)))
}
