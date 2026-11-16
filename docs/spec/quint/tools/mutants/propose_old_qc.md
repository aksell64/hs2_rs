# propose_old_qc

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step

`consensus.qnt`, `on_propose`: a Byzantine leader builds its block on the genesis QC instead of `high_qc`.

Status: success; deviates in almost every trace (299 of 300), also with `step_fair` and together with `vote_past_lock`.

```diff
--- consensus.qnt
-      val parent = kv.block_by_hash.get(high_qc.block)
+      val genesis_qc = match get_block_by_hash(kv, 0) { | Some(g) => g.justify | None => high_qc }
+      val qc = if (is_byzantine(c.id)) genesis_qc else high_qc
+      val parent = kv.block_by_hash.get(qc.block)
-      val block = create_leaf(request.hash, parent.hash, high_qc, parent.height + 1)
+      val block = create_leaf(request.hash, parent.hash, qc, parent.height + 1)
       {
-        consensus: c.with("proposed_view", request.view),
+        consensus: c.with("proposed_view", request.view).with("deviations", c.deviations + (if (qc.view < high_qc.view) 1 else 0)),
```
