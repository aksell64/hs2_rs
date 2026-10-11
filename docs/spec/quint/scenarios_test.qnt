module scenariosTest {
  import types.* from "./types"
  import driver.* from "./driver"
  import effects.* from "./effects"
  import network.* from "./network"
  import state_machine.* from "./sm"
  import pacemaker.* from "./pacemaker"
  import invariants.* from "./invariants"
  import storage.* from "./storage"
  import main(committee_size = 4, FAULTY = Set(0), BYZANTINE = Set(), MAX_VIEW = 6, FAULTY_PERCENT = 0, TIMER_PERCENT = 0).* from "./main"

  action unchanged: bool = all {
    nodes' = nodes,
    prev_nodes' = nodes,
    net' = net,
    qc_views' = qc_views,
    all_qcs' = all_qcs,
    syncs' = syncs,
    sync_net' = sync_net,
    minted' = minted,
  }

  action inject(receiver: NodeID, packet: Packet): bool = all {
    nodes' = nodes.put(receiver, push_to_queue(nodes.get(receiver), packet_to_input(packet))),
    prev_nodes' = nodes,
    net' = net,
    qc_views' = qc_views,
    all_qcs' = all_qcs,
    syncs' = syncs,
    sync_net' = sync_net,
    minted' = minted.union(match packet.msg { | ProposalMsg(proposal) => Set(proposal.block) | _ => Set() }),
  }

  def view_of(id: NodeID): View = nodes.get(id).driver.pacemaker.cur_view
  def voted_of(id: NodeID): View = nodes.get(id).driver.consensus.voted_view
  def high_qc_of(id: NodeID): CertifiedBlock = nodes.get(id).driver.pacemaker.get_high_qc()
  def lock_of(id: NodeID): CertifiedBlock = nodes.get(id).driver.consensus.qc_lock

  // safety_inv contains transition invariants that compare with prev_nodes, i.e. only with
  // the previous state, so it is checked after every step.
  run checked_step(id: NodeID): bool =
    apply_step(id).expect(safety_inv)

  // A step of `id` that turns into a no-op once the leader of view v has proposed.
  action step_until_proposed(id: NodeID, v: View): bool =
    if (leader_finished(v)) unchanged else apply_step(id)

  run round_until_proposed(v: View): bool =
    step_until_proposed(1, v).expect(safety_inv)
      .then(step_until_proposed(2, v)).expect(safety_inv)
      .then(step_until_proposed(3, v)).expect(safety_inv)

  // Upper bound on rounds for one view on the happy path. The busiest node is the next leader:
  // it handles the proposal, at most committee_size votes and its EnterView, one event per step,
  // and may lag one round behind each sender. Rounds after the stop condition are no-ops,
  // so the bound only has to be large enough.
  pure val MAX_ROUNDS_PER_VIEW = 2 * committee_size + 2

  // Runs the correct nodes until the leader of view v + 1 has built QC(v) and proposed in v + 1.
  // Expects the leader of v + 1 to be correct; checks that every correct node voted in v
  // and that safety holds.
  run happy_view(v: View): bool =
    MAX_ROUNDS_PER_VIEW.reps(_ => round_until_proposed(v + 1))
      .expect(CORRECT.contains(leader(v + 1)))
      .expect(leader_finished(v + 1))
      .expect(high_qc_of(leader(v + 1)).view == v)
      .expect(view_of(leader(v + 1)) == v + 1)
      .expect(CORRECT.forall(id => voted_of(id) == v))

  // From the initial state, runs happy_view(1), ..., happy_view(v - 1): views 1, ..., v - 1 are
  // completed, view v is only started. Expects the leaders of views 2, ..., v to be correct.
  //
  // Stops right after the leader of v has proposed in v. At that point:
  //   - QC(1), ..., QC(v - 1) exist; QC(v - 1) is known only to the leader of v, which built it
  //     from the votes;
  //   - the leader of v is in view v and has broadcast Proposal(v) with justify = QC(v - 1),
  //     it has not handled its own proposal yet;
  //   - every other correct node is in view v - 1, has voted in v - 1 and has Proposal(v)
  //     unhandled in its inbox;
  //   - nobody has voted in v, so QC(v) does not exist.
  run happy_until(v: View): bool =
    (v - 1).reps(i => happy_view(i + 1))

  // n0 is faulty and silent; views 1, 2 and 3 have correct leaders n1, n2 and n3.
  run happyPathTest =
    init.then(happy_until(3))

  def proposal(proposer: NodeID, version: int, view: View, justify: CertifiedBlock): Proposal =
    {
      block: {
        hash: block_variant_hash(view, proposer, version),
        parent: justify.block,
        height: all_blocks(nodes).get(justify.block).height + 1,
        justify: justify,
      },
      view: view,
    }

  // A rejected proposal is still stored and passed to update, so update alone must keep the lock
  // from going down. After view 2 n1 is locked on QC(1) and has Proposal(v3) of n3 in its inbox.
  // n1 also gets a second proposal of n3 for view 3 built on the genesis QC (equivocation; n3 is
  // correct in this module, the test only exercises the replica). The fake one is buffered until
  // view 3, the real one moves n1 to view 3 with a vote and the lock QC(2), then the fake one is
  // replayed, rejected (already voted in view 3) and passed to update, which must keep QC(2).
  run lockNeverDecreasesTest =
    init
      .then(happy_until(3))
      .expect(lock_of(1).view == 1)
      .then(inject(1, { sender: 3, msg: ProposalMsg(proposal(3, 1, 3, GENESIS_QC)) }))
      .then(6.reps(_ => checked_step(1)))
      .expect(view_of(1) == 3)
      .expect(voted_of(1) == 3)
      .expect(lock_of(1).view == 2)
      // The fake block reached update: blocks are stored only together with update.
      .expect(nodes.get(1).kv.block_by_hash.keys().contains(block_variant_hash(3, 3, 1)))

  // After view 1 n2 holds QC(1) and has broadcast Proposal(v2). The faulty n0 sends n1 its own
  // proposal for view 2 built on the same QC(1), although the leader of view 2 is n2. n1 handles
  // the fake one first (it is at the queue head), drops it without processing its QC, and then
  // votes for the real proposal of n2.
  run proposalFromNonLeaderIsIgnoredTest =
    init
      .then(happy_view(1))
      .then(inject(1, { sender: 0, msg: ProposalMsg(proposal(0, 1, 2, high_qc_of(2))) }))
      .then(apply_step(1))
      .expect(voted_of(1) == 1)
      .expect(view_of(1) == 1)
      .expect(high_qc_of(1).view == 0)
      .then(apply_step(1))
      .expect(voted_of(1) == 2)
      .expect(view_of(1) == 2)

  // A replica does not vote for a block whose justify is below its lock. After view 2 n1 is locked
  // on QC(1) and has Proposal(v3) of n3 in its inbox. n1 also gets a second proposal of n3 for view 3
  // built on the genesis QC (equivocation; n3 is correct in this module). Its view timer moves it to
  // view 3 before it handles the real proposal: the fake one is buffered, n1 times out of view 2,
  // enters view 3, the fake one is replayed and must be rejected; then n1 votes for the real one.
  run voteRespectsLockTest =
    init
      .then(happy_until(3))
      .expect(lock_of(1).view == 1)
      .then(inject(1, { sender: 3, msg: ProposalMsg(proposal(3, 1, 3, GENESIS_QC)) }))
      .then(fire_view_timer(1))
      .then(3.reps(_ => checked_step(1)))
      // The fake block was handled in view 3: it is stored, and n1 did not vote for it.
      .expect(view_of(1) == 3)
      .expect(nodes.get(1).kv.block_by_hash.keys().contains(block_variant_hash(3, 3, 1)))
      .expect(voted_of(1) == 2)
      .then(checked_step(1))
      .expect(voted_of(1) == 3)
      .expect(lock_of(1).view == 2)

  // Blocksync. Pure checks of the request gate and of the driver handling of a synced block.
  pure val qc_of_b1: CertifiedBlock = { certificate: { signature: QuorumCert(Set(1, 2, 3)) }, block: block_hash(1, 1), view: 1 }
  pure val b1: Block = { hash: block_hash(1, 1), parent: 0, height: 1, justify: GENESIS_QC }
  pure val b2: Block = { hash: block_hash(2, 2), parent: block_hash(1, 1), height: 2, justify: qc_of_b1 }
  pure val genesis_kv = insert_block(empty_storage, GENESIS)

  pure def proposal_of(sender: NodeID, block: Block, view: View): DriverInput =
    NewProposal({ sender: sender, proposal: { block: block, view: view } })

  // A missing parent is requested only for a proposal of the leader whose parent is certified by
  // its own valid QC.
  run syncGateTest =
    val leader_proposal = proposal_of(2, b2, 2)
    val bad_qc = b2.with("justify", qc_of_b1.with("certificate", { signature: QuorumCert(Set(1)) }))
    val other_parent = b2.with("justify", GENESIS_QC)
    init
      .expect(check_missing_parent(genesis_kv, leader_proposal) == MissingBlock({ qc: qc_of_b1, parked: [leader_proposal] }))
      .expect(check_missing_parent(genesis_kv, proposal_of(1, b2, 2)) == NoSmOutput)
      .expect(check_missing_parent(genesis_kv, proposal_of(2, bad_qc, 2)) == NoSmOutput)
      .expect(check_missing_parent(genesis_kv, proposal_of(2, other_parent, 2)) == NoSmOutput)
      .expect(check_missing_parent(insert_block(genesis_kv, b1), leader_proposal) == NoSmOutput)

  // A synced block is stored and its QC is taken, but nobody votes for it.
  run blockSyncedStoresWithoutVoteTest =
    val d = init_driver(1, GENESIS, GENESIS_QC)
    val synced = apply_driver(d, insert_block(genesis_kv, b1), BlockSynced(b2))
    val orphan = apply_driver(d, genesis_kv, BlockSynced(b2))
    init
      .expect(synced.kv.block_by_hash.keys().contains(b2.hash))
      .expect(synced.driver.pacemaker.get_high_qc() == qc_of_b1)
      .expect(synced.net.length() == 0)
      .expect(synced.driver.consensus.voted_view == 0)
      .expect(not(orphan.kv.block_by_hash.keys().contains(b2.hash)))
}
