module pacemaker {
  import types.* from "./types"
  import effects.* from "./effects"

  type PacemakerInput =
  | ReceivedQC(CertifiedBlock)
  | ViewTimerExpired(View)
  | LockTimerExpired(View)
  | ReceivedTC(CertifiedView)
  | ReceivedStatus({ sender: NodeID, qc: CertifiedBlock, view: View })

  type PacemakerOutput =
  | NoPacemakerOutput
  | ProposeNow(View)

  type Pacemaker = {
    id: NodeID,
    high_qc: CertifiedBlock,
    cur_view: View,
    cur_leader: NodeID,
    proposed: bool,
    statuses: Set[NodeID],
    sent_wish: Set[View],
    relayed_tc: Set[View],
    scheduled_epoch: Option[int],
  }

  type PacemakerResult = {
    pacemaker: Pacemaker,
    effects: List[Effect],
    out: PacemakerOutput,
  }

  pure def init_pacemaker(id: NodeID, genesis_qc: CertifiedBlock): Pacemaker =
    {
      id: id,
      high_qc: genesis_qc,
      cur_view: 0,
      cur_leader: leader(0),
      proposed: false,
      statuses: Set(id),
      sent_wish: Set(),
      relayed_tc: Set(),
      scheduled_epoch: None,
    }

  pure def leader(view: View): NodeID = view % committee_size

  pure def get_current_view(pcm: Pacemaker): View = pcm.cur_view

  pure def get_high_qc(pcm: Pacemaker): CertifiedBlock = pcm.high_qc

  pure def get_leader_by_view(pcm: Pacemaker, view: View): NodeID = leader(view)

  pure def is_leader(pcm: Pacemaker, view: View): bool = pcm.id == leader(view)

  pure val epoch_length: int = faulty(committee_size) + 1

  pure def epoch_leaders(view: View): Set[NodeID] =
    0.to(epoch_length - 1).map(k => leader(view - view % epoch_length + k))

  pure def epoch_of(view: View): int = view / epoch_length

  pure def is_new_epoch(pcm: Pacemaker, epoch: int): bool =
    match pcm.scheduled_epoch {
      | Some(e) => epoch > e
      | None => true
    }

  pure def epoch_timers(v: View): List[Effect] =
    val end = v - v % epoch_length + epoch_length
    range(v, end).foldl([], (acc, w) => acc.append(Timer(SetViewTimer(w))))

  pure def apply_pacemaker(pcm: Pacemaker, input: PacemakerInput): PacemakerResult =
    match input {
      | ReceivedQC(qc) => process_qc(pcm, qc)
      | ViewTimerExpired(v) => on_timer_expired(pcm, v)
      | LockTimerExpired(v) => on_lock_timer_expired(pcm, v)
      | ReceivedTC(tc) => on_receive_tc(pcm, tc)
      | ReceivedStatus(status) => on_receive_status(pcm, status)
    }

  pure def pacemaker_no_change(pcm: Pacemaker): PacemakerResult =
    { pacemaker: pcm, effects: [], out: NoPacemakerOutput }

  pure def with_effects(effects: List[Effect], r: PacemakerResult): PacemakerResult =
    r.with("effects", effects.concat(r.effects))

  pure def process_qc(pcm: Pacemaker, qc: CertifiedBlock): PacemakerResult =
    if (not(is_valid_qc(qc)) or qc.view <= pcm.high_qc.view)
      pacemaker_no_change(pcm)
    else
      val updated = pcm.with("high_qc", qc)
      if (qc.view >= updated.cur_view)
        advance_view(updated, qc.view + 1)
      else
        pacemaker_no_change(updated)

  pure def advance_view(pcm: Pacemaker, v: View): PacemakerResult =
    if (v <= pcm.cur_view)
      pacemaker_no_change(pcm)
    else
      val epoch = epoch_of(v)
      val schedule = if (is_new_epoch(pcm, epoch)) epoch_timers(v) else []
      val moved = pcm
        .with("cur_view", v)
        .with("cur_leader", leader(v))
        .with("proposed", false)
        .with("statuses", Set(pcm.id))
        .with("scheduled_epoch", Some(epoch))
      val entered = enter_view(moved, v)
      val timers = [Timer(ClearLockTimer(pcm.cur_view)), Timer(ClearViewTimersBelow(v))].concat(schedule)
      with_effects(timers, entered)

  pure def enter_view(pcm: Pacemaker, v: View): PacemakerResult =
    if (v != pcm.cur_view)
      pacemaker_no_change(pcm)
    else if (pcm.id == leader(v))
      if (pcm.high_qc.view == v - 1)
        try_propose(pcm)
      else
        { pacemaker: pcm, effects: [Timer(SetLockTimer(v))], out: NoPacemakerOutput }
    else if (pcm.high_qc.view != v - 1)
      val status = send_to(Node(leader(v)), StatusMsg({ qc: pcm.high_qc, view: v }))
      { pacemaker: pcm, effects: [status], out: NoPacemakerOutput }
    else
      pacemaker_no_change(pcm)

  pure def try_propose(pcm: Pacemaker): PacemakerResult =
    if (pcm.proposed)
      pacemaker_no_change(pcm)
    else
      { pacemaker: pcm.with("proposed", true), effects: [], out: ProposeNow(pcm.cur_view) }

  pure def on_lock_timer_expired(pcm: Pacemaker, v: View): PacemakerResult =
    if (v == pcm.cur_view and pcm.id == leader(v))
      try_propose(pcm)
    else
      pacemaker_no_change(pcm)

  pure def on_timer_expired(pcm: Pacemaker, v: View): PacemakerResult =
    if (v != pcm.cur_view)
      pacemaker_no_change(pcm)
    else if ((v + 1) % epoch_length != 0)
      advance_view(pcm, v + 1)
    else if (pcm.sent_wish.contains(v + 1))
      pacemaker_no_change(pcm)
    else
      val wishes = send_to_each(epoch_leaders(v + 1), WishMsg(v + 1))
      val sent = pcm.with("sent_wish", pcm.sent_wish.union(Set(v + 1)))
      { pacemaker: sent, effects: wishes, out: NoPacemakerOutput }

  pure def on_receive_tc(pcm: Pacemaker, tc: CertifiedView): PacemakerResult =
    if (not(is_valid_tc(tc)) or pcm.relayed_tc.contains(tc.view))
      pacemaker_no_change(pcm)
    else
      val relayed = pcm.with("relayed_tc", pcm.relayed_tc.union(Set(tc.view)))
      val relay =
        if (epoch_leaders(tc.view).contains(pcm.id)) [send_to(All, TCMsg(tc))]
        else send_to_each(epoch_leaders(tc.view), TCMsg(tc))
      val advanced = advance_view(relayed, tc.view)
      with_effects(relay, advanced)

  pure def on_receive_status(
      pcm: Pacemaker,
      status: { sender: NodeID, qc: CertifiedBlock, view: View }): PacemakerResult =
    if (pcm.id == leader(status.view))
      process_qc(pcm, status.qc)
    else
      pacemaker_no_change(pcm)
}
