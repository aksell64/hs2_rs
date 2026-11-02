1) Смена валидаторов внутри пайплайна
2) hotstuff2 (two chain + optimistically resposiveness)
3) Lumiere pacemaker
4) Tail-forking в конвейере: византийский L(v+1) выбрасывает голоса за блок L(v). Изучить защиту:
   reproposal из MonadBFT (tip в таймаутах, NEC) и carry-блоки (HotStuff-1, arXiv 2508.12173 «Carry the Tail in Consensus Protocols»).
   В Quint-модели видно: пары блоков на одной высоте после views 4k+2 (голоса уходят византийцу 3).
5) Доказательство safety для любого числа нод и view (Quint проверяет только ограниченные трассы).
   Veil (Lean 4 + SMT, в духе Ivy): абстрактная модель на отношениях (voted, qc, justify, ancestor,
   кворумы с аксиомой пересечения), индуктивный инвариант усиливаем по контрпримерам к индукции (CTI).
   Кандидаты в усиления уже есть в main.qnt: once_qc_in_view_inv, ancestry_inv, lock_protection_inv.
   Главная трудность — поздние голоса: QC(V+1) может собраться после QC старших view, поэтому
   инвариант формулируется для ещё собираемых кворумов (как в «Paxos Made EPR»).
   Опционально Aeneas (Rust → Lean) для чистых функций: update lock, safe_node, commit rule.



Задачи:

1) трейты для подписей/qc +
2) трейт для сети
3) трейт для kv
4) I11 (invariants.md): в драйвере StoreBlock сохраняет блок, только если родитель есть и
   height = parent.height + 1; из незакоммиченных хранить только потомков b_exec (закоммиченная
   цепочка до b_exec остаётся). BlockSynced без родителя отбрасывать.
5) I11: в commit_up_to обрыв ветки до exec_hash — не Ok(None), а KVError::Corruption.
6) I12 (invariants.md): qc_high только на сохранённый блок. Варианты: обновлять qc_high только при
   сохранённом блоке, а view продвигать по любому валидному QC; или повторять ProcessQC после того,
   как blocksync догрузил блок.
7) Сеть в Quint: FIFO отдельно по каждому отправителю вместо одной очереди на получателя.
   inbox: NodeID -> (NodeID -> List[Packet]), apply_step(id, from) с nondet from среди отправителей
   с непустой очередью (как TCP-соединение на пира). Сейчас родитель всегда приходит раньше потомка,
   и MissingBlock/BlockSynced в симуляциях не срабатывают (n4f1v20: 0 запросов за 1000 шагов).
   Затронет network.qnt, sm.qnt (pop_from_network), main.qnt (has_messages, step/step_fair/step_happy),
   scenarios_test.qnt (apply_step), tools/analyze.py (inbox_of), MBT-обвязку (ящики по отправителям,
   SpecNet). В Rust нужен мок сети с перестановкой между отправителями, иначе e2e тоже не задевает
   blocksync.
8) + Баг: proposal с отсутствующим родителем теряется. sm.rs, apply_next/check_missing_parent:
   при NewProposal без родителя в хранилище уходит только sync родителя, сам proposal выбрасывается
   и после BlockSynced не переигрывается. Отставший узел навсегда остаётся на блок позади, не голосует,
   его view истекают по таймауту. Воспроизводит synchronous_network_after_uneven_entrance (старты
   10/20/30/40: высота 19 вместо 91); в byzantine_leader_withholds_proposals_from_one_replica
   прогресс в 4 раза медленнее. Исправление: парковать proposal по хешу родителя и вернуть в очередь
   после BlockSynced. Заодно: check_missing_parent стоит до проверок отправителя и QC, поэтому
   любой узел может запустить sync произвольного хеша — парковать только после этих проверок.
9) Баг: wish отправляется один раз, таймер повторно не взводится. Если wish потерялся или
   адресат ещё не стартовал, view-change не завершается. При широком разбросе стартов
   (n1@300, n2@600, n3@900) ни одного коммита и полная тишина после t=1010. Исправление:
   повторять wish (или status) по таймеру, пока не получен TC/QC старшего view.
10) + Баг: лидер с QC(v−1) без блока под ним молча не предлагал блок в v, view терялся по таймауту.
   Исправлено: Orphans хранит отложенные действия по хешу недостающего блока (Deferred::Propose,
   Deferred::ReceiveProposal), on_propose возвращает ProposalDeferred и запрашивает sync, действия
   выполняются после BlockSynced.
11) Оптимизация: сохранять устаревшие предложения через RejectVote (on_receive_proposal) и снимать
   отложенные действия при любом StoreBlock, а не только на BlockSynced. Экономит запрос sync и Δ.
   Делать только вместе: serve_requests в blocksync.qnt молча снимает запрос, если блок уже есть,
   и без снятия при StoreBlock отложенное действие зависнет.

Проверить:

1) onReceiveQC до onReceiveProposal: на входе NewProposal драйвер проверяет leader(view) и verifyQC(justify),
   затем сразу [ProcessQC(justify), ProposalOrBuffer(p)]. Уходят ConsensusEvent::ReceivedProposal,
   ConsensusOutput::ReceiveQC и on_receive_proposal; в driver.qnt:on_proposal на один match меньше.
   Ловушка: on_process_qc возвращает None и для невалидного, и для устаревшего QC, поэтому verifyQC
   должен стоять в драйвере, иначе ProposalOrBuffer пропустит поддельный justify.
2) Совместить advance и enter (убрать trigger enterView): за одно событие не больше одного advance,
   а эффекты enter не пересекаются с ProposalOrBuffer(p). Варианты: в call_advance_view вместо
   add_out(NewView) вызвать call_enter_view; или on_view_timer_started сам вызывает enter_view и
   возвращает его выход вместо EnterNewView (ReleasePending остаётся в драйвере). Уходят
   DriverOutput::NewView, DriverEvent::EnterView, ветка NewView в sm. Поправить protocol.md,
   driver/pacemaker/sm.qnt, scenarios_test.qnt (MAX_ROUNDS_PER_VIEW), Rust.
3) Расхождение protocol.md и Quint/Rust в построении блока. protocol.md: конвейер на b_leaf —
   createLeaf(b_leaf, cmd, qc_high, …), b_leaf ← b_new, следующий блок поверх несертифицированного;
   проверка justify.node ⪯ b_new (justify может указывать на дальнего предка). Quint/Rust:
   parent = high_qc.block, justify = high_qc (on_propose), extends_justify требует
   parent == justify.block — у сертифицированных блоков justify всегда на прямого родителя.
   Решить, какой вариант основной; от этого зависят R5 в контракте blocksync (invariants.md)
   и проверка ancestry.
   Решение: строгий вариант (как в Quint/Rust). При ротации лидера на каждый view у честных
   justify.node = parent всегда, кроме угла: тот же лидер вёл v−1, его блок не набрал QC, и в Case 2
   b_leaf — его несертифицированный блок. В protocol.md: createLeaf(qc_high.node, …), убрать b_leaf,
   проверка b_new.justify.node = b_new.parent. Убирать b_leaf и ужесточать проверку только вместе,
   иначе честное предложение в этом углу отвергается (liveness). Следствия: b'.parent = b в update
   избыточна — мутанты no_direct_link_equivocate / no_direct_link_tail_fork (mutants.toml):
   safety_inv ок, direct_link_mattered_witness 0 из 300; R5 в контракте blocksync следует из verifyQC.
