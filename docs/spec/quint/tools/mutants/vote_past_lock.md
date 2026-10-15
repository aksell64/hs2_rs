# vote_past_lock

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step

`consensus.qnt`, `can_vote`: the Byzantine node votes regardless of its lock.

Status: success, but almost vacuously: correct leaders rarely propose below a lock, the Byzantine node deviated in 1–6 of 300 traces with `step` and never with `step_fair`.

```diff
--- consensus.qnt
-c.voted_view < view and block.justify.view >= c.qc_lock.view
+c.voted_view < view and (block.justify.view >= c.qc_lock.view or is_byzantine(c.id))

--- consensus.qnt
-consensus: c.with("voted_view", cur_view),
+consensus: c.with("voted_view", cur_view)
+          .with("deviations", c.deviations + (if (block.justify.view < c.qc_lock.view) 1 else 0)),
```
