module pacemaker {
  import types.* from "./types"

  type PacemakerInput =
  | ProcessQC(CertifiedBlock)
  | HighQCApplied(CertifiedBlock)
  | TimerExpiredView(View)
  | LockTimerExpired(View)
  | EnterViewRequested(View)
  | AdvanceViewRequested(View)
  | LockTimerCanceled(View)
  | ViewTimerStarted(View)
  | ReceiveWish({ sender: NodeID, view: View })
  | TimeoutQC(CertifiedView)
  | ReceiveStatus({ sender: NodeID, qc: CertifiedBlock, view: View })

  type PacemakerOutput =
  | NoPacemakerOutput
  | HighQCUpdated(CertifiedBlock)
  | StartTimerView(View)
  | StartLockTimer(View)
  | CancelLockTimer({ view: View, next: View })
  | ProposeNow(View)
  | SendStatus({ to: NodeID, qc: CertifiedBlock, view: View })
  | AdvanceView(View)
  | SendWish({ to: Set[NodeID], view: View })
  | EnterNewView(View)
  | ReceiveWishIsLeader({ sender: NodeID, view: View })
  | BroadcastTC(CertifiedView)
  | SendTCToLeader({ to: Set[NodeID], tc: CertifiedView })
  | StatusIsLeader({ sender: NodeID, qc: CertifiedBlock, view: View })

  type Pacemaker = {
    id: NodeID,
    high_qc: CertifiedBlock,
    cur_view: View,
    cur_leader: NodeID,
    proposed: bool,
    statuses: Set[NodeID],
    sent_wish: Set[View],
    relayed_tc: Set[View],
  }

  type PacemakerResult = {
    pacemaker: Pacemaker,
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
    }

  pure def leader(view: View): NodeID = view % committee_size

  pure def get_current_view(pcm: Pacemaker): View = pcm.cur_view

  pure def get_high_qc(pcm: Pacemaker): CertifiedBlock = pcm.high_qc

  pure def get_leader_by_view(pcm: Pacemaker, view: View): NodeID = leader(view)

  pure def is_leader(pcm: Pacemaker, view: View): bool = pcm.id == leader(view)

  pure val epoch_length: int = faulty(committee_size) + 1

  pure def epoch_leaders(view: View): Set[NodeID] =
    0.to(epoch_length - 1).map(k => leader(view - view % epoch_length + k))

  pure def apply_pacemaker(pcm: Pacemaker, input: PacemakerInput): PacemakerResult =
    match input {
      | ProcessQC(qc) => on_process_qc(pcm, qc)
      | HighQCApplied(qc) => on_high_qc_applied(pcm, qc)
      | EnterViewRequested(v) => enter_view(pcm, v)
      | LockTimerExpired(v) => on_lock_timer_expired(pcm, v)
      | TimerExpiredView(v) => on_timer_expired(pcm, v)
      | AdvanceViewRequested(v) => on_advance_view(pcm, v)
      | LockTimerCanceled(v) => on_lock_timer_canceled(pcm, v)
      | ViewTimerStarted(v) => on_view_timer_started(pcm, v)
      | ReceiveWish(wish) => on_receive_wish(pcm, wish)
      | TimeoutQC(tc) => on_receive_tc(pcm, tc)
      | ReceiveStatus(status) => on_receive_status(pcm, status)
    }

  pure def pacemaker_no_change(pcm: Pacemaker): PacemakerResult =
    { pacemaker: pcm, out: NoPacemakerOutput }

  pure def on_process_qc(pcm: Pacemaker, qc: CertifiedBlock): PacemakerResult =
    if (is_valid_qc(qc) and qc.view > pcm.high_qc.view)
      { pacemaker: pcm.with("high_qc", qc), out: HighQCUpdated(qc) }
    else
      pacemaker_no_change(pcm)

  pure def enter_view(pcm: Pacemaker, v: View): PacemakerResult =
    if (v != pcm.cur_view)
      pacemaker_no_change(pcm)
    else if (pcm.id == leader(v))
      if (pcm.high_qc.view == v - 1)
        try_propose(pcm)
      else
        { pacemaker: pcm, out: StartLockTimer(v) }
    else if (pcm.high_qc.view != v - 1)
      { pacemaker: pcm, out: SendStatus({ to: leader(v), qc: pcm.high_qc, view: v }) }
    else
      pacemaker_no_change(pcm)

  pure def try_propose(pcm: Pacemaker): PacemakerResult =
    if (pcm.proposed)
      pacemaker_no_change(pcm)
    else
      { pacemaker: pcm.with("proposed", true), out: ProposeNow(pcm.cur_view) }

  pure def on_lock_timer_expired(pcm: Pacemaker, v: View): PacemakerResult =
    if (v == pcm.cur_view and pcm.id == leader(v))
      try_propose(pcm)
    else
      pacemaker_no_change(pcm)

  pure def on_timer_expired(pcm: Pacemaker, v: View): PacemakerResult =
    if (v != pcm.cur_view)
      pacemaker_no_change(pcm)
    else if ((v + 1) % epoch_length != 0)
      { pacemaker: pcm, out: AdvanceView(v + 1) }
    else if (pcm.sent_wish.contains(v + 1))
      pacemaker_no_change(pcm)
    else
      {
        pacemaker: pcm.with("sent_wish", pcm.sent_wish.union(Set(v + 1))),
        out: SendWish({ to: epoch_leaders(v + 1), view: v + 1 }),
      }

  pure def on_advance_view(pcm: Pacemaker, v: View): PacemakerResult =
    if (v <= pcm.cur_view)
      pacemaker_no_change(pcm)
    else
      {
        pacemaker: pcm
          .with("cur_view", v)
          .with("cur_leader", leader(v))
          .with("proposed", false)
          .with("statuses", Set(pcm.id)),
        out: CancelLockTimer({ view: pcm.cur_view, next: v }),
      }

  pure def on_lock_timer_canceled(pcm: Pacemaker, v: View): PacemakerResult =
    if (v != pcm.cur_view)
      pacemaker_no_change(pcm)
    else
      { pacemaker: pcm, out: StartTimerView(v) }

  pure def on_view_timer_started(pcm: Pacemaker, v: View): PacemakerResult =
    if (v != pcm.cur_view)
      pacemaker_no_change(pcm)
    else
      { pacemaker: pcm, out: EnterNewView(v) }

  pure def on_receive_wish(pcm: Pacemaker, wish: { sender: NodeID, view: View }): PacemakerResult =
    if (epoch_leaders(wish.view).contains(pcm.id))
      { pacemaker: pcm, out: ReceiveWishIsLeader(wish) }
    else
      pacemaker_no_change(pcm)

  pure def on_receive_tc(pcm: Pacemaker, tc: CertifiedView): PacemakerResult =
    if (not(is_valid_tc(tc)) or pcm.relayed_tc.contains(tc.view))
      pacemaker_no_change(pcm)
    else
      val relayed = pcm.with("relayed_tc", pcm.relayed_tc.union(Set(tc.view)))
      if (epoch_leaders(tc.view).contains(pcm.id))
        { pacemaker: relayed, out: BroadcastTC(tc) }
      else
        { pacemaker: relayed, out: SendTCToLeader({ to: epoch_leaders(tc.view), tc: tc }) }

  pure def on_receive_status(
      pcm: Pacemaker,
      status: { sender: NodeID, qc: CertifiedBlock, view: View }): PacemakerResult =
    if (pcm.id == leader(status.view))
      { pacemaker: pcm, out: StatusIsLeader(status) }
    else
      pacemaker_no_change(pcm)

  pure def on_high_qc_applied(pcm: Pacemaker, qc: CertifiedBlock): PacemakerResult =
    if (qc.view >= pcm.cur_view)
      { pacemaker: pcm, out: AdvanceView(qc.view + 1) }
    else
      pacemaker_no_change(pcm)
}
