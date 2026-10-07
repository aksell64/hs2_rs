module main {
  import types.* from "./types"
  import storage.* from "./storage"
  import driver.* from "./driver"
  import network.* from "./network"
  import pacemaker.* from "./pacemaker"
  import state_machine.* from "./sm"
  import invariants.* from "./invariants"
  import blocksync.* from "./blocksync"

  pure val NODES: Set[NodeID] = 0.to(committee_size - 1)
  const FAULTY: Set[NodeID]
  pure val CORRECT: Set[NodeID] = NODES.exclude(FAULTY)
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

  pure def init_node(id: NodeID): NodeState =
    {
      node_id: id,
      driver: init_driver(id, GENESIS, GENESIS_QC),
      kv: insert_block(empty_storage, GENESIS),
      queue: [ViewTimerFired(0)],
      view_timer: None,
      lock_timer: None,
    }

  action init = all {
    nodes' = CORRECT.mapBy(id => init_node(id)),
    prev_nodes' = CORRECT.mapBy(id => init_node(id)),
    net' = { inbox: CORRECT.mapBy(_ => []) },
    qc_views' = record_qc(Map(), GENESIS_QC),
    all_qcs' = Set(GENESIS_QC),
    syncs' = CORRECT.mapBy(_ => init_blocksync),
  }

  action apply_step(id: NodeID): bool =
    val node = nodes.get(id)
    val r = apply_state(node, net)
    all {
      nodes' = nodes.put(id, r.state),
      prev_nodes' = nodes,
      net' = r.net,
      qc_views' = record_qcs_of(qc_views, r.state),
      all_qcs' = all_qcs.union(qcs_of(r.state)),
      syncs' = match r.out {
        | MissingBlock(hash) => syncs.put(id, request_block(syncs.get(id), hash))
        | NoSmOutput => syncs
      },
    }

  action sync_step(id: NodeID): bool =
    val node = nodes.get(id)
    val res = serve_requests(syncs.get(id), node.kv, all_blocks(nodes))
    all {
      res.blocksync != syncs.get(id),
      nodes' = nodes.put(id, node.with("queue", res.inputs.concat(node.queue))),
      prev_nodes' = nodes,
      net' = net,
      qc_views' = qc_views,
      all_qcs' = all_qcs,
      syncs' = syncs.put(id, res.blocksync),
    }

  action enqueue_timer(id: NodeID, node: NodeState, input: DriverInput): bool = all {
    nodes' = nodes.put(id, push_to_queue(node, input)),
    prev_nodes' = nodes,
    net' = net,
    qc_views' = qc_views,
    all_qcs' = all_qcs,
    syncs' = syncs,
  }

  pure def armed_view(timer: Option[View]): View =
    match timer {
      | Some(v) => v
      | None => -1
    }

  // The lock timer (P_pc + Δ) is shorter than the view slot (τ), so a leader waiting
  // in Case 2 always gets to propose before its view times out. Without clocks this
  // order is lost; the guard below restores it.
  action fire_view_timer(id: NodeID): bool =
    val node = nodes.get(id)
    all {
      node.view_timer != None,
      node.lock_timer == None,
      enqueue_timer(id, node.with("view_timer", None), ViewTimerFired(armed_view(node.view_timer))),
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
    }
  }

  action step_no_timers = {
    nondet id = CORRECT.oneOf()
    any {
      apply_step(id),
      deliver_faulty(id),
      sync_step(id),
    }
  }

  def has_messages(id: NodeID): bool =
    nodes.get(id).queue.length() > 0 or net.inbox.get(id).length() > 0

  def has_sync_requests(id: NodeID): bool =
    syncs.get(id).requested.size() > 0

  def has_armed_timer(id: NodeID): bool =
    nodes.get(id).view_timer != None or nodes.get(id).lock_timer != None

  def actors(dice: int): Set[NodeID] =
    CORRECT.filter(id => or {
      has_messages(id),
      has_sync_requests(id),
      dice < TIMER_PERCENT and has_armed_timer(id),
      dice < FAULTY_PERCENT,
    })

  action step = {
    nondet dice = 0.to(99).oneOf()
    val candidates = actors(dice)
    nondet id = (if (candidates.size() > 0) candidates else CORRECT).oneOf()
    any {
      all { has_messages(id) or candidates.size() == 0, apply_step(id) },
      all { dice < FAULTY_PERCENT, deliver_faulty(id) },
      all { dice < TIMER_PERCENT, fire_view_timer(id) },
      all { dice < TIMER_PERCENT, fire_lock_timer(id) },
      sync_step(id),
    }
  }

  def leader_finished(v: View): bool =
    val l = leader(v)
    if (FAULTY.contains(l))
      true
    else
      val pcm = nodes.get(l).driver.pacemaker
      pcm.cur_view > v or (pcm.cur_view == v and pcm.proposed)

  // After GST the view slot τ outlasts everything an honest leader needs to propose
  // and deliver its proposal, so a view timer only expires once the node has drained
  // its messages and the leader of that view has proposed or moved on.
  def view_timer_ready_gst(id: NodeID): bool =
    val node = nodes.get(id)
    and {
      node.view_timer != None,
      node.lock_timer == None,
      not(has_messages(id)),
      leader_finished(armed_view(node.view_timer)),
    }

  def actors_gst(dice: int): Set[NodeID] =
    CORRECT.filter(id => or {
      has_messages(id),
      has_sync_requests(id),
      view_timer_ready_gst(id),
      nodes.get(id).lock_timer != None,
      dice < FAULTY_PERCENT,
    })

  action step_gst = {
    nondet dice = 0.to(99).oneOf()
    val candidates = actors_gst(dice)
    nondet id = (if (candidates.size() > 0) candidates else CORRECT).oneOf()
    any {
      all { has_messages(id) or candidates.size() == 0, apply_step(id) },
      all { dice < FAULTY_PERCENT, deliver_faulty(id) },
      all { view_timer_ready_gst(id), fire_view_timer(id) },
      fire_lock_timer(id),
      sync_step(id),
    }
  }

  val agreement_inv = agreement(nodes)

  val lock_monotonic_inv = CORRECT.forall(id => monotony_of_lock(nodes.get(id), prev_nodes.get(id)))

  val voted_view_monotonic_inv = CORRECT.forall(id => monotony_of_voted_view(nodes.get(id), prev_nodes.get(id)))

  val high_qc_monotonic_inv = CORRECT.forall(id => monotony_of_high_qc(nodes.get(id), prev_nodes.get(id)))

  val exec_monotonic_inv = CORRECT.forall(id => monotony_of_executed_blocks(nodes.get(id), prev_nodes.get(id)))

  val once_qc_in_view_inv = once_qc_in_view(qc_views)

  val lock_protection_inv = lock_protection(qc_views, all_blocks(nodes))

  val ancestry_inv = certified_extends_justify(qc_views, all_blocks(nodes))

  val safety_inv = and {
    agreement_inv,
    lock_monotonic_inv,
    voted_view_monotonic_inv,
    high_qc_monotonic_inv,
    exec_monotonic_inv,
    once_qc_in_view_inv,
    lock_protection_inv,
    ancestry_inv,
  }

  val no_commit_witness = CORRECT.forall(id => nodes.get(id).kv.committed_branch.length() == 0)
}
