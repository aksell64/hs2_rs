module invariants {
  import types.* from "./types"
  import storage.* from "./storage"
  import driver.* from "./driver"
  import network.* from "./network"
  import pacemaker.* from "./pacemaker"
  import state_machine.* from "./sm"

  pure def monotony_of_lock(cur_state: NodeState, prev_state: NodeState): bool =
    cur_state.driver.consensus.qc_lock.view >= prev_state.driver.consensus.qc_lock.view

  pure def monotony_of_voted_view(cur_state: NodeState, prev_state: NodeState): bool =
    cur_state.driver.consensus.voted_view >= prev_state.driver.consensus.voted_view

  pure def monotony_of_high_qc(cur_state: NodeState, prev_state: NodeState): bool =
    cur_state.driver.pacemaker.get_high_qc().view >= prev_state.driver.pacemaker.get_high_qc().view

  pure def monotony_of_executed_blocks(cur_state: NodeState, prev_state: NodeState): bool =
    cur_state.driver.consensus.exec_height >= prev_state.driver.consensus.exec_height


  type QCViewCollector = View -> Set[Hash]

  pure def record_qc(c: QCViewCollector, qc: CertifiedBlock): QCViewCollector =
    val blocks = if (c.keys().contains(qc.view)) c.get(qc.view) else Set()
    c.put(qc.view, blocks.union(Set(qc.block)))

  pure def qcs_of(state: NodeState): Set[CertifiedBlock] =
    val blocks = state.kv.block_by_hash
    Set(state.driver.pacemaker.get_high_qc(), state.driver.consensus.qc_lock)
      .union(blocks.keys().map(h => blocks.get(h).justify))

  pure def record_qcs_of(c: QCViewCollector, state: NodeState): QCViewCollector =
    qcs_of(state).fold(c, (acc, qc) => record_qc(acc, qc))

  pure def once_qc_in_view(c: QCViewCollector): bool =
    c.keys().forall(v => c.get(v).size() <= 1)

  pure def all_blocks(nodes: NodeID -> NodeState): Hash -> Block =
    nodes.keys().fold(Map(), (acc, id) =>
      val kv = nodes.get(id).kv.block_by_hash
      kv.keys().fold(acc, (m, h) => m.put(h, kv.get(h))))

  pure def is_ancestor(blocks: Hash -> Block, anc: Hash, desc: Hash): bool =
    if (not(blocks.keys().contains(anc)) or not(blocks.keys().contains(desc)))
      false
    else
      val depth = blocks.get(desc).height - blocks.get(anc).height
      if (depth < 0)
        false
      else
        val reached = range(0, depth).foldl(Some(desc), (cur, _) => match cur {
          | Some(h) => if (blocks.keys().contains(h)) Some(blocks.get(h).parent) else None
          | None => None
        })
        reached == Some(anc)

  pure def directly_committed(qcs: QCViewCollector, blocks: Hash -> Block, v: View, b: Hash): bool =
    qcs.keys().contains(v + 1) and
      qcs.get(v + 1).exists(c => blocks.keys().contains(c) and blocks.get(c).parent == b)

  pure def lock_protection(qcs: QCViewCollector, blocks: Hash -> Block): bool =
    qcs.keys().forall(v => qcs.get(v).forall(b =>
      directly_committed(qcs, blocks, v, b) implies
        qcs.keys().forall(w => w >= v implies qcs.get(w).forall(x => is_ancestor(blocks, b, x)))))

  pure def certified_extends_justify(qcs: QCViewCollector, blocks: Hash -> Block): bool =
    qcs.keys().forall(v => qcs.get(v).forall(b =>
      blocks.keys().contains(b) and is_ancestor(blocks, blocks.get(b).justify.block, b)))

  pure def is_prefix(a: List[Block], b: List[Block]): bool =
    a.length() <= b.length() and a.indices().forall(i => a[i] == b[i])

  pure def agreement(nodes: NodeID -> NodeState): bool =
    nodes.keys().forall(i => nodes.keys().forall(j =>
      val ci = nodes.get(i).kv.committed_branch
      val cj = nodes.get(j).kv.committed_branch
      is_prefix(ci, cj) or is_prefix(cj, ci)
    ))
}
