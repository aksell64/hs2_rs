```
Event-driven Chained HotStuff-2 (for replica u)
// n = 3f + 1 is the fixed committee size; f is the upper bound on Byzantine nodes.
// Each epoch contains n views.

// Pseudocode assumptions:
//   - all blocks are already synchronized: every block's parent and ancestors are in storage
//     (fetching missing blocks is handled by blocksync, outside this file);
//   - nodes process events synchronously: every broadcast is processed atomically by all
//     recipients immediately, so proposals from future views do not arrive
//     (buffering deferred proposals is handled by the implementation, outside this file).

// State:
//   b_lock, b_exec        — the locked block and the last executed block
//   qc_lock               — the QC that locks b_lock (lock rank is its view)
//   qc_high               — the highest known QC (ranked by view)
//   votedView             — the last view in which u voted
//   curView               — the current view
//   proposed              — whether u has already proposed in curView
//   scheduledEpoch        — the last epoch with a schedule (⊥ before the first one)
//   V[b, view]            — vote shares; W[v] — wish shares
//   M                     — status senders for the current view, including u itself
//   sentWish[v], relayedTC[v] — duplicate suppression
//
//   init: b_lock = b_exec = b0 (genesis); qc_lock = qc_high = QC(b0);
//         votedView = 0; curView = 0; scheduledEpoch = ⊥;
//         startup: ViewTimerFired(0) fires immediately at every node → advanceView(1)
//
//   verifyQC(qc)          — qc contains n−f valid shares for ⟨qc.node, qc.viewNumber⟩
//   verifyTC(tc)          — tc contains 2f+1 valid wish shares for tc.view
//   verifyShare(σ, msg, p) — σ is a valid share from p for msg

// ───────────────────────── Consensus ─────────────────────────

upon NewProposal(proposalMsg_v(generic, b_new, view))
    if v ≠ getLeader(view) then return
    if ¬verifyQC(b_new.justify) then return
    onReceiveQC(b_new.justify)
    if view ≠ curView then return
    if b_new.justify.node ≠ b_new.parent then return
    if b_new.height ≠ b_new.parent.height + 1 then return
    if votedView < curView
       ∧ b_new.justify.viewNumber ≥ qc_lock.viewNumber then
        votedView ← curView
        send(getLeader(curView + 1), voteMsg_u(generic, b_new, curView))
    update(b_new)

upon NewVote(m = voteMsg_v(generic, b, view))
    if u ≠ getLeader(view + 1) then return
    if ¬verifyShare(m.partialSig, ⟨generic, b, view⟩, v) then return
    if ∃⟨v, σ'⟩ ∈ V[b, view] then return
    V[b, view] ← V[b, view] ∪ {⟨v, m.partialSig⟩}
    if |V[b, view]| ≥ n − f then
        qc ← QC({σ | ⟨v, σ⟩ ∈ V[b, view]}, view)
        onReceiveQC(qc)

procedure update(b*)
    // The caller must invoke onReceiveQC(b*.justify) BEFORE update.
    // update handles only locking and commits; it does not advance views.
    b' ← b*.justify.node ; b ← b'.justify.node
    if b*.justify.viewNumber > qc_lock.viewNumber then
        b_lock ← b'; qc_lock ← b*.justify
    if b'.parent = b
       ∧ b*.justify.viewNumber = b'.justify.viewNumber + 1
       ∧ b.height > b_exec.height then
        onCommit(b); b_exec ← b

procedure onCommit(b)
    if b_exec.height < b.height then
        onCommit(b.parent); execute(b.cmd)

procedure tryPropose()
    if proposed then return
    proposed ← true
    onPropose(cmd)

procedure onPropose(cmd)
    b_new ← createLeaf(qc_high.node, cmd, qc_high, qc_high.node.height + 1)
    broadcast(proposalMsg_u(generic, b_new, curView))

function createLeaf(parent, cmd, qc, height)
    b.parent ← parent; b.cmd ← cmd
    b.justify ← qc; b.height ← height; return b

// ───────────────────────── Pacemaker ─────────────────────────

upon LockTimerFired(v)
    if v = curView ∧ u = getLeader(v) then tryPropose()

upon ViewTimerFired(v)
    if v ≠ curView then return
    if (v + 1) mod n ≠ 0 then
        advanceView(v + 1)
    else
        if ¬sentWish[v + 1] then
            sentWish[v + 1] ← true
            σ ← createThresholdShare(⟨wish, v + 1⟩)
            send wishMsg_u(v + 1, σ) to epochLeaders(v + 1)

upon NewWish(m = wishMsg_v(view, σ))
    if u ∉ epochLeaders(view) then return
    if ¬verifyShare(σ, ⟨wish, view⟩, v) then return
    if ∃⟨v, σ'⟩ ∈ W[view] then return
    W[view] ← W[view] ∪ {⟨v, σ⟩}
    if |W[view]| ≥ 2f + 1 then
        tc ← TC(view, {σ | ⟨p, σ⟩ ∈ W[view]})
        processTC(tc)

upon NewTC(tc)
    processTC(tc)

upon NewQC(qc)
    onReceiveQC(qc)

upon NewStatus(m = statusMsg_v(qc, view))
    if u ≠ getLeader(view) then return
    onReceiveQC(qc)
    if view = curView then
        M ← M ∪ {v}
        if |M| = n then onHeardAll(curView)

procedure onReceiveQC(qc)
    if ¬verifyQC(qc) then return
    if qc.viewNumber > qc_high.viewNumber then
        qc_high ← qc
    if qc.viewNumber ≥ curView then
        nextView ← qc.viewNumber + 1
        if nextView mod n = 0 then
            broadcast(msg_u(QC, qc))
        advanceView(nextView)

procedure setTimers(v)
    e ← v − (v mod n)
    a ← now
    for w ← v … (e + n − 1) do
        schedule ViewTimerFired(w) at a + (w − e + 1)·τ

procedure advanceView(v')
    if v' ≤ curView then return
    cancel timer case2(curView)
    cancel timers ViewTimerFired(w) for w < v'
    curView ← v'; proposed ← false
    M ← {u}
    if scheduledEpoch = ⊥ ∨ epochOf(v') > scheduledEpoch then
        scheduledEpoch ← epochOf(v')
        setTimers(v')
    enterView(v')

procedure enterView(v)
    if v ≠ curView then return
    // Enter (Figure 1)
    if u = getLeader(curView) then
        if qc_high.viewNumber = curView − 1 then
            tryPropose()
        else
            start timer case2(curView) at now + P_pc + Δ
    else
        if qc_high.viewNumber ≠ curView − 1 then
            send statusMsg_u(qc_high, curView) to getLeader(curView)

procedure onHeardAll(v)
    if v = curView ∧ u = getLeader(v) then
        cancel timer case2(v)
        tryPropose()

procedure processTC(tc)
    if ¬verifyTC(tc) then return
    if relayedTC[tc.view] then return
    relayedTC[tc.view] ← true
    if u ∈ epochLeaders(tc.view) then
        broadcast(msg_u(TC, tc))
    else
        send msg_u(TC, tc) to epochLeaders(tc.view)
    advanceView(tc.view)

function getLeader(view)
function epochLeaders(view)
function epochOf(view) = view div n
```
