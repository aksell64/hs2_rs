module scenariosTest {
  import types.* from "./types"
  import driver.* from "./driver"
  import effects.* from "./effects"
  import network.* from "./network"
  import state_machine.* from "./sm"
  import pacemaker.* from "./pacemaker"
  import invariants.* from "./invariants"
  import main(committee_size = 4, FAULTY = Set(0), BYZANTINE = Set(), MAX_VIEW = 6, FAULTY_PERCENT = 0, TIMER_PERCENT = 0).* from "./main"

  action unchanged: bool = all {
    nodes' = nodes,
    prev_nodes' = nodes,
    net' = net,
    qc_views' = qc_views,
    all_qcs' = all_qcs,
    syncs' = syncs,
  }

  action inject(receiver: NodeID, packet: Packet): bool = all {
    nodes' = nodes.put(receiver, push_to_queue(nodes.get(receiver), packet_to_input(packet))),
    prev_nodes' = nodes,
    net' = net,
    qc_views' = qc_views,
    all_qcs' = all_qcs,
    syncs' = syncs,
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

  run epochBoundaryWaitsForTcTest =
    val initial = init_pacemaker(1, GENESIS_QC)
    val at1 = apply_pacemaker(initial, ViewTimerExpired(0))
    val at2 = apply_pacemaker(at1.pacemaker, ViewTimerExpired(1))
    val at3 = apply_pacemaker(at2.pacemaker, ViewTimerExpired(2))
    val waiting = apply_pacemaker(at3.pacemaker, ViewTimerExpired(3))
    val repeated = apply_pacemaker(waiting.pacemaker, ViewTimerExpired(3))
    val tc = { certificate: { signature: QuorumCert(CORRECT) }, view: 4 }
    val entered = apply_pacemaker(waiting.pacemaker, ReceivedTC(tc))
    all {
      at1.pacemaker.cur_view == 1,
      1.to(3).forall(v => at1.effects.select(effect => effect == Timer(SetViewTimer(v))).length() == 1),
      at2.pacemaker.cur_view == 2,
      at2.pacemaker.sent_wish == Set(),
      at3.pacemaker.cur_view == 3,
      at3.pacemaker.sent_wish == Set(),
      waiting.pacemaker.cur_view == 3,
      waiting.pacemaker.sent_wish == Set(4),
      waiting.effects == send_to_each(NODES, WishMsg(4)),
      repeated.effects == [],
      entered.pacemaker.cur_view == 4,
      entered.pacemaker.scheduled_epoch == Some(1),
      entered.effects.select(effect => effect == send_to(All, TCMsg(tc))).length() == 1,
      4.to(7).forall(v => entered.effects.select(effect => effect == Timer(SetViewTimer(v))).length() == 1),
    }

  pure val EPOCH_BOUNDARY_QC: CertifiedBlock = {
    certificate: { signature: QuorumCert(CORRECT) },
    block: block_hash(3, 3),
    view: 3,
  }

  run epochQcRelayedOnceTest =
    val qc2 = EPOCH_BOUNDARY_QC.with("view", 2).with("block", block_hash(2, 2))
    val at3 = apply_pacemaker(init_pacemaker(1, GENESIS_QC), ReceivedQC(qc2))
    val entered = apply_pacemaker(at3.pacemaker, ReceivedQC(EPOCH_BOUNDARY_QC))
    val repeated = apply_pacemaker(entered.pacemaker, ReceivedQC(EPOCH_BOUNDARY_QC))
    all {
      at3.pacemaker.cur_view == 3,
      entered.pacemaker.cur_view == 4,
      entered.pacemaker.high_qc == EPOCH_BOUNDARY_QC,
      entered.pacemaker.scheduled_epoch == Some(1),
      entered.effects.head() == send_to(All, QCMsg(EPOCH_BOUNDARY_QC)),
      entered.effects.select(effect => effect == send_to(All, QCMsg(EPOCH_BOUNDARY_QC))).length() == 1,
      4.to(7).forall(v => entered.effects.select(effect => effect == Timer(SetViewTimer(v))).length() == 1),
      repeated == pacemaker_no_change(entered.pacemaker),
    }

  run qcInsideEpochIsNotRelayedTest =
    val qc2 = EPOCH_BOUNDARY_QC.with("view", 2).with("block", block_hash(2, 2))
    val qc5 = EPOCH_BOUNDARY_QC.with("view", 5).with("block", block_hash(5, 1))
    val at3 = apply_pacemaker(init_pacemaker(1, GENESIS_QC), ReceivedQC(qc2))
    val at6 = apply_pacemaker(at3.pacemaker, ReceivedQC(qc5))
    all {
      at3.pacemaker.cur_view == 3,
      at3.effects.select(effect => effect == send_to(All, QCMsg(qc2))).length() == 0,
      at6.pacemaker.cur_view == 6,
      at6.pacemaker.scheduled_epoch == Some(1),
      at6.effects.select(effect => effect == send_to(All, QCMsg(qc5))).length() == 0,
    }

  run epochQcAfterTcIsNotRelayedTest =
    val tc = { certificate: { signature: QuorumCert(CORRECT) }, view: 4 }
    val entered = apply_pacemaker(init_pacemaker(1, GENESIS_QC), ReceivedTC(tc))
    val received = apply_pacemaker(entered.pacemaker, ReceivedQC(EPOCH_BOUNDARY_QC))
    all {
      received.pacemaker.cur_view == 4,
      received.pacemaker.high_qc == EPOCH_BOUNDARY_QC,
      received.effects == [],
      received.out == NoPacemakerOutput,
    }

  run invalidEpochQcIsIgnoredTest =
    val initial = init_pacemaker(1, GENESIS_QC)
    Set(InvalidCert, QuorumCert(Set(1, 2))).forall(signature =>
      val invalid = EPOCH_BOUNDARY_QC.with("certificate", { signature: signature })
      apply_pacemaker(initial, ReceivedQC(invalid)) == pacemaker_no_change(initial))

  // The faulty collector n0 reveals QC(3) only to n1 after receiving all honest votes.
  // Honest nodes relay the QC and enter view 4 through message delivery alone.
  run epochQcReachesAllHonestNodesTest =
    val vote = VoteMsg({ view: EPOCH_BOUNDARY_QC.view, block: EPOCH_BOUNDARY_QC.block })
    val message = QCMsg(EPOCH_BOUNDARY_QC)
    init
      .then(happy_until(3))
      .then(checked_step(1))
      .then(checked_step(2))
      .then(checked_step(3))
      .expect(CORRECT.forall(id => view_of(id) == 3 and voted_of(id) == 3))
      .expect(CORRECT.forall(id => net.inbox.get(0).select(packet => packet == { sender: id, msg: vote }).length() == 1))
      .then(inject(1, { sender: 0, msg: message }))
      .then(checked_step(1))
      .expect(view_of(1) == 4 and view_of(2) == 3 and view_of(3) == 3)
      .expect(Set(2, 3).forall(id => net.inbox.get(id).select(packet => packet == { sender: 1, msg: message }).length() == 1))
      .then(checked_step(2))
      .then(checked_step(3))
      .expect(CORRECT.forall(id => view_of(id) == 4 and high_qc_of(id) == EPOCH_BOUNDARY_QC))
      // Drain self-delivery and duplicates to check that rebroadcasting terminates.
      .then(CORRECT.size().reps(_ => checked_step(1).then(checked_step(2)).then(checked_step(3))))
      .expect(CORRECT.forall(id => not(has_messages(id)) and view_of(id) == 4))

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

  def vote_packet(sender: NodeID, view: View, block: Hash): Packet =
    { sender: sender, msg: VoteMsg({ view: view, block: block }) }

  def stored(id: NodeID, hash: Hash): bool =
    nodes.get(id).kv.block_by_hash.keys().contains(hash)

  run orphanProposalVotesAfterSyncTest =
    val h1 = block_hash(1, 1)
    val h2 = block_hash(2, 2)
    init
      .then(inject(2, vote_packet(0, 1, h1)))
      .then(MAX_ROUNDS_PER_VIEW.reps(_ => step_until_proposed(1, 2).then(step_until_proposed(2, 2))))
      .expect(leader_finished(2))
      .then(inject(3, { sender: 2, msg: ProposalMsg(proposal(2, 0, 2, high_qc_of(2))) }))
      .then(2.reps(_ => checked_step(3)))
      .expect(view_of(3) == 2)
      .expect(voted_of(3) == 0)
      .expect(not(stored(3, h1)))
      .expect(not(stored(3, h2)))
      .expect(has_sync_requests(3))
      .expect(nodes.get(3).driver.orphans.keys() == Set(h1))
      .then(sync_step(3))
      .then(checked_step(3))
      .expect(stored(3, h1))
      .expect(stored(3, h2))
      .expect(voted_of(3) == 2)
      .expect(nodes.get(3).driver.orphans.keys() == Set())

  run leaderProposesAfterSyncTest =
    val h1 = block_hash(1, 1)
    init
      .then(2.reps(_ => checked_step(1)))
      .expect(stored(1, h1))
      .then(inject(2, vote_packet(0, 1, h1)))
      .then(inject(2, vote_packet(1, 1, h1)))
      .then(inject(2, vote_packet(3, 1, h1)))
      .then(4.reps(_ => checked_step(2)))
      .expect(view_of(2) == 2)
      .expect(high_qc_of(2).view == 1)
      .expect(not(stored(2, h1)))
      .expect(not(leader_finished(2)))
      .expect(has_sync_requests(2))
      .expect(nodes.get(2).driver.orphans.keys() == Set(h1))
      .then(sync_step(2))
      .then(checked_step(2))
      .expect(stored(2, h1))
      .expect(leader_finished(2))
      .expect(nodes.get(2).driver.orphans.keys() == Set())
      .expect(net.inbox.get(3).select(p => p.msg == ProposalMsg(proposal(2, 0, 2, high_qc_of(2)))).length() == 1)
}
