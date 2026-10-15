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
    nodes' = RUNNING.mapBy(id => init_node(id)),
    prev_nodes' = RUNNING.mapBy(id => init_node(id)),
    net' = { inbox: RUNNING.mapBy(_ => []) },
    qc_views' = record_qc(Map(), GENESIS_QC),
    all_qcs' = Set(GENESIS_QC),
    syncs' = RUNNING.mapBy(_ => init_blocksync),
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

  def has_messages(id: NodeID): bool =
    nodes.get(id).queue.length() > 0 or net.inbox.get(id).length() > 0

  def has_sync_requests(id: NodeID): bool =
    syncs.get(id).requested.size() > 0

  def has_armed_timer(id: NodeID): bool =
    nodes.get(id).view_timer != None or nodes.get(id).lock_timer != None

  def actors(dice: int): Set[NodeID] =
    RUNNING.filter(id => or {
      has_messages(id),
      has_sync_requests(id),
      dice < TIMER_PERCENT and has_armed_timer(id),
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
      node.view_timer != None,
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
      leader_finished(armed_view(node.view_timer)),
    }

  def actors_fair(dice: int): Set[NodeID] =
    RUNNING.filter(id => or {
      has_messages(id),
      has_sync_requests(id),
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
      sync_step(id),
    }
  }

  action step_happy = {
    val candidates = RUNNING.filter(id => has_messages(id) or has_sync_requests(id))
    nondet id = (if (candidates.size() > 0) candidates else RUNNING).oneOf()
    any {
      apply_step(id),
      sync_step(id),
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

  // Tail-forking: the block certified in view v is dropped by the certified block of view v + 1.
  val tail_fork_witness =
    val blocks = all_blocks(nodes)
    qc_views.keys().exists(v => qc_views.keys().contains(v + 1) and
      qc_views.get(v).exists(b => qc_views.get(v + 1).exists(c =>
        blocks.keys().contains(c) and blocks.get(c).parent != b)))
}
