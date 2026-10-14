```
Event-driven Chained HotStuff-2 (for replica u)
// Ранги, локи, правило голосования, коммит (соседние views), dual regime и
// pacemaker — всё на views (HotStuff-2, Figures 1–2). Высота блока — только
// глубина цепи для порядка исполнения; в решениях consensus не участвует.
// View блока не хранится: в message (предложение) и в QC (сертифицированные).

// State:
//   b_lock, b_exec        — залоченный и последний исполненный блок
//   qc_lock              — QC, которым залочен b_lock (ранг лока по view)
//   qc_high              — высший известный QC (ранг по view)
//   b_leaf                — лист для построения предложения; двигается и чужим
//                           сертификатом, и собственным предложением (конвейер).
//                           b_leaf.height ≥ qc_high.node.height, равенство не обязательно.
//   votedView             — последний view, в котором u голосовал
//   curView               — текущий view
//   proposed              — предлагал ли u уже в curView
//   scheduled             — эпохи, для которых расписание уже построено
//   V[b, view]            — доли голосов; W[v] — доли wish
//   M                     — отправители status текущего view, включая самого u
//   sentWish[v], relayedTC[v] — защита от повторов
//
//   init: b_lock = b_exec = b_leaf = b0 (genesis); qc_lock = qc_high = QC(b0);
//         votedView = 0; curView = 0
//
//   verifyQC(qc)              — qc несёт n−f валидных долей за ⟨qc.node, qc.viewNumber⟩
//   verifyTC(tc)             — tc несёт 2t+1 валидных wish-долей за tc.view
//   verifyShare(σ, msg, p)   — σ — валидная доля p за msg

// ───────────────────────── Consensus ─────────────────────────

procedure createLeaf(parent, cmd, qc, height)
    b.parent ← parent; b.cmd ← cmd
    b.justify ← qc; b.height ← height; return b

procedure update(b*)
    // caller обязан вызвать onReceiveQC(b*.justify) ДО update.
    // update отвечает только за лок и коммит; продвижение по views — не его дело.
    b' ← b*.justify.node ; b ← b'.justify.node
    if b*.justify.viewNumber > qc_lock.viewNumber then       // lock на 1-chain, ранг по view
        b_lock ← b'; qc_lock ← b*.justify                    // b*.justify = QC(b'), им и лочимся
    if b'.parent = b                                          // commit на 2-chain: прямое звено
       ∧ b*.justify.viewNumber = b'.justify.viewNumber + 1   //   И соседние views (QC(b'), QC(b) подряд)
       ∧ b.height > b_exec.height then                        //   И только вперёд — не откатывать b_exec
        onCommit(b); b_exec ← b                              // конвейерные блоки коммитятся как предки

procedure onCommit(b)
    if b_exec.height < b.height then
        onCommit(b.parent); execute(b.cmd)

procedure onReceiveProposal(msg_v(generic, b_new, view))
    if v ≠ getLeader(view) then return                       // предлагает лидер своего view
    if ¬verifyQC(b_new.justify) then return                  // justify — настоящий QC, иначе весь proposal невалиден
    onReceiveQC(b_new.justify)                               // [I1] ПЕРВОЙ, до проверок view. TODO: мб сначала вызывать pacemaker OnReceiveQC, а потом onReceiveProposal без OnReceiveQC
    if view ≠ curView then return
    if ¬(b_new.justify.node ⪯ b_new) then return             // ancestry: b_new продолжает свой justify
                                                             //   (нет предков до justify.node — догрузить, не голосовать)
    if b_new.height ≠ b_new.parent.height + 1 then return    // W1: высота = высота родителя + 1
    if votedView < curView                                   // [I6] один голос за попытку
       ∧ b_new.justify.viewNumber ≥ qc_lock.viewNumber then  // ранг justify не ниже лока (по view)
        votedView ← curView
        send(getLeader(curView + 1), voteMsg_u(generic, b_new, curView))
    update(b_new)

procedure onReceiveVote(m = voteMsg_v(generic, b, view))
    if u ≠ getLeader(view + 1) then return                   // голоса адресованы мне
    if ¬verifyShare(m.partialSig, ⟨generic, b, view⟩, v) then return  // валидная доля от v (не отравить аккумулятор)
    if ∃⟨v, σ'⟩ ∈ V[b, view] then return                     // [I4] дедуп по отправителю
    V[b, view] ← V[b, view] ∪ {⟨v, m.partialSig⟩}            // [I5] ключ по паре ⟨b, view⟩
    if |V[b, view]| ≥ n − f then
        qc ← QC({σ | ⟨v, σ⟩ ∈ V[b, view]}, view)             // qc.viewNumber ← view
        onReceiveQC(qc)                                      // → advanceView(view + 1)

function onPropose(cmd)
    b_new ← createLeaf(b_leaf, cmd, qc_high, b_leaf.height + 1)
    broadcast(msg_u(generic, b_new, curView))                // включая себя; view предложения — в message
    b_leaf ← b_new                                           // конвейер: следующее предложение
                                                             // строится поверх несертифицированного

procedure tryPropose()
    if proposed then return                                  // не более одного предложения за view
    proposed ← true
    onPropose(cmd)

// ───────────── Мост: единственная точка приёма QC ─────────────

procedure onReceiveQC(qc)
    if ¬verifyQC(qc) then return                             // сертификат должен быть настоящим
    if qc.viewNumber > qc_high.viewNumber then               // ранг по view (не по высоте!)
        qc_high ← qc; b_leaf ← qc.node
    if qc.viewNumber ≥ curView then
        advanceView(qc.viewNumber + 1)                        // Advance по сертификату

procedure onReceiveStatus(m = statusMsg_v(qc, view))
    if u ≠ getLeader(view) then return
    onReceiveQC(qc)                                          // сначала: verify + может продвинуть curView
    if view = curView then                                   // проверка против нового curView
        M ← M ∪ {v}
        if |M| = n then onHeardAll(curView)

// ───────────────────────── Pacemaker ─────────────────────────

function getLeader(view)                                     // specified by the application
function epochLeaders(view)                                  // t+1 лидеров эпохи этого view
function epochOf(view) = view div (t + 1)

procedure setTimers(v)                                       // вход в эпоху через ЛЮБОЙ её view
    e ← v − (v mod (t + 1))                                  // первый view эпохи
    T ← now                                                  // now = начало слота для view v
    for k ← (v − e) … t do                                   // пропущенные слоты не ставим
        schedule onTimerExpired(e + k) at T + (k − (v − e) + 1)·τ

procedure advanceView(v')                                    // синхронно: только состояние
    if v' ≤ curView then return
    cancel timer case2(curView)                              // снимаем таймер покидаемого view
    curView ← v'; proposed ← false
    M ← {u}                                                  // u учитывает себя
    if epochOf(v') ∉ scheduled then                          // расписание один раз на эпоху
        scheduled ← scheduled ∪ {epochOf(v')}
        setTimers(v')
    trigger enterView(v')                                    // асинхронно: эффекты

procedure enterView(v)                                       // отдельный обработчик
    if v ≠ curView then return                               // устаревший триггер
    // Enter (Figure 1)
    if u = getLeader(curView) then
        if qc_high.viewNumber = curView − 1 then              // Case 1: responsive
            tryPropose()
        else                                                 // Case 2: прошлый view сломан
            start timer case2(curView) at now + P_pc + Δ
    else
        if qc_high.viewNumber ≠ curView − 1 then              // вошёл не по сертификату
            send statusMsg_u(qc_high, curView) to getLeader(curView)

procedure onCase2Timer(v)                                    // истёк case2(v)
    if v = curView ∧ u = getLeader(v) then tryPropose()

procedure onHeardAll(v)                                      // |M| = n при v = curView
    if v = curView ∧ u = getLeader(v) then
        cancel timer case2(v)
        tryPropose()

procedure onTimerExpired(v)                                  // слот kτ для view v
    if v ≠ curView then return                               // устаревший таймер
    if (v + 1) mod (t + 1) ≠ 0 then                          // v не последний в эпохе
        advanceView(v + 1)                                   // локально, без коммуникации
    else                                                     // epoch synchronization
        if ¬sentWish[v + 1] then
            sentWish[v + 1] ← true
            σ ← createThresholdShare(⟨wish, v + 1⟩)
            send ⟨wish, v + 1, σ⟩ to epochLeaders(v + 1)

procedure onReceiveWish(m = ⟨wish, v', σ⟩ from p)
    if u ∉ epochLeaders(v') then return
    if ¬verifyShare(σ, ⟨wish, v'⟩, p) then return            // валидная доля от p
    if ∃⟨p, σ'⟩ ∈ W[v'] then return                          // дедуп по отправителю
    W[v'] ← W[v'] ∪ {⟨p, σ⟩}
    if |W[v']| ≥ 2t + 1 then
        tc ← TC(v', {σ | ⟨p, σ⟩ ∈ W[v']})
        onReceiveTC(tc)

procedure onReceiveTC(tc)
    if ¬verifyTC(tc) then return                             // сертификат должен быть настоящим
    if relayedTC[tc.view] then return
    relayedTC[tc.view] ← true
    if u ∈ epochLeaders(tc.view) then
        broadcast(msg_u(TC, tc))                             // лидер: всем
    else
        send msg_u(TC, tc) to epochLeaders(tc.view)          // party: relay
    advanceView(tc.view)
```