# tail_fork

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step
witnesses: tail_fork_witness

`consensus.qnt`, `on_propose`: a Byzantine leader of v + 1 builds on its lock QC(v − 1) instead of `high_qc` = QC(v), dropping the block of view v.

Status: success; deviates in 295 of 300 traces, the fork gets a QC (`tail_fork_witness`) in 276 of 300 (24 of 300 with all nodes correct).

```diff
--- consensus.qnt
-    match get_block_by_hash(kv, high_qc.block) {
+    val qc = if (is_byzantine(c.id)) c.qc_lock else high_qc
+    match get_block_by_hash(kv, qc.block) {
       | None => no_change(c)
       | Some(parent) =>
-        val block = create_leaf(request.hash, parent.hash, high_qc, parent.height + 1)
+        val block = create_leaf(request.hash, parent.hash, qc, parent.height + 1)
         {
-          consensus: c,
+          consensus: c.with("deviations", c.deviations + (if (qc.view < high_qc.view) 1 else 0)),
```
