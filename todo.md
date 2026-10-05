1) Смена валидаторов внутри пайплайна
2) hotstuff2 (two chain + optimistically resposiveness)
3) Lumiere pacemaker
4) Tail-forking в конвейере: византийский L(v+1) выбрасывает голоса за блок L(v). Изучить защиту:
   reproposal из MonadBFT (tip в таймаутах, NEC) и carry-блоки (HotStuff-1, arXiv 2508.12173 «Carry the Tail in Consensus Protocols»).
   В Quint-модели видно: пары блоков на одной высоте после views 4k+2 (голоса уходят византийцу 3).



Задачи:

1) трейты для подписей/qc +
2) трейт для сети
3) трейт для kv