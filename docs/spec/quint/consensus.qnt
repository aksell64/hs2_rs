module consensus {
  import types.* from "./types"
  import storage.* from "./storage"

  type ProposeRequest = {
    hash: Hash,
    view: View,
  }

  type ConsensusInput =
  | ReceivedProposal(Proposal)
  | QCProcessed(Proposal)
  | Update(Block)
  | MakeProposal(ProposeRequest)

  type ConsensusOutput =
  | NoOutput
  | ReceiveQC(CertifiedBlock)
  | ConfirmVote(GenericVote)
  | RejectVote(Block)
  | Commit(List[Block])
  | Propose(Proposal)

  type Consensus = {
    qc_lock: CertifiedBlock,
    voted_view: View,
    exec_hash: Hash,
    exec_height: Height,
  }

  type ConsensusResult = {
    consensus: Consensus,
    out: ConsensusOutput,
  }

  pure def init_consensus(genesis: Block, genesis_qc: CertifiedBlock): Consensus =
    {
      qc_lock: genesis_qc,
      voted_view: 0,
      exec_hash: genesis.hash,
      exec_height: genesis.height,
    }

  pure def apply_consensus(
      c: Consensus,
      kv: KVStorage,
      cur_view: View,
      high_qc: CertifiedBlock,
      input: ConsensusInput): ConsensusResult =
    match input {
      | ReceivedProposal(proposal) => on_receive_proposal(c, proposal)
      | QCProcessed(proposal) => on_qc_processed(c, kv, cur_view, proposal)
      | Update(block) => update(c, block, kv)
      | MakeProposal(request) => on_propose(c, kv, high_qc, request)
    }

  pure def no_change(c: Consensus): ConsensusResult =
    { consensus: c, out: NoOutput }

  pure def on_receive_proposal(c: Consensus, proposal: Proposal): ConsensusResult =
    if (is_valid_qc(proposal.block.justify))
      { consensus: c, out: ReceiveQC(proposal.block.justify) }
    else
      no_change(c)

  pure def on_qc_processed(
      c: Consensus,
      kv: KVStorage,
      cur_view: View,
      proposal: Proposal): ConsensusResult =
    val block = proposal.block
    if (proposal.view != cur_view or not(extends_justify(block, kv)))
      no_change(c)
    else if (can_vote(c, block, cur_view))
      {
        consensus: c.with("voted_view", cur_view),
        out: ConfirmVote({ view: cur_view, block: block.hash }),
      }
    else
      { consensus: c, out: RejectVote(block) }

  pure def extends_justify(block: Block, kv: KVStorage): bool =
    block.parent == block.justify.block
      and match get_block_by_hash(kv, block.parent) {
        | Some(parent) => block.height == parent.height + 1
        | None => false
      }

  pure def can_vote(c: Consensus, block: Block, view: View): bool =
    c.voted_view < view and block.justify.view >= c.qc_lock.view

  pure def update(c: Consensus, b_star: Block, kv: KVStorage): ConsensusResult =
    val locked =
      if (b_star.justify.view > c.qc_lock.view)
        c.with("qc_lock", b_star.justify)
      else
        c

    match get_block_by_hash(kv, b_star.justify.block) {
      | None => no_change(locked)
      | Some(b_prime) =>
        match get_block_by_hash(kv, b_prime.justify.block) {
          | None => no_change(locked)
          | Some(b) =>
            if (b_prime.parent == b.hash
                and b_star.justify.view == b_prime.justify.view + 1
                and b.height > locked.exec_height)
              commit_up_to(locked, b, kv)
            else
              no_change(locked)
        }
    }

  pure def commit_up_to(c: Consensus, target: Block, kv: KVStorage): ConsensusResult =
    val depth = target.height - c.exec_height
    val branch = get_block_branch(target, kv, depth)
    if (branch.length() != depth or branch[depth - 1].parent != c.exec_hash)
      no_change(c)
    else
      {
        consensus: c.with("exec_hash", target.hash).with("exec_height", target.height),
        out: Commit(range(0, depth).foldl([], (acc, i) => acc.append(branch[depth - 1 - i]))),
      }

  pure def on_propose(
      c: Consensus,
      kv: KVStorage,
      high_qc: CertifiedBlock,
      request: ProposeRequest): ConsensusResult =
    match get_block_by_hash(kv, high_qc.block) {
      | None => no_change(c)
      | Some(parent) =>
        val block = create_leaf(request.hash, parent.hash, high_qc, parent.height + 1)
        {
          consensus: c,
          out: Propose({ block: block, view: request.view }),
        }
    }
}
