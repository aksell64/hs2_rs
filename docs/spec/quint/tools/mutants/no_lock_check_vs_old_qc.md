# no_lock_check_vs_old_qc

expect: violation
byzantine: 3
committee_size: 4
max_view: 20
invariant: lock_protection_inv
step: step

`no_lock_check` on all nodes and `propose_old_qc` on the Byzantine node.

Status: success: with a leader that proposes on the genesis QC, correct replicas without the lock check certify a fork of a committed block (also with `step_fair`).

```diff
--- consensus.qnt
-c.voted_view < view and block.justify.view >= c.qc_lock.view
+c.voted_view < view

--- consensus.qnt
-    match get_block_by_hash(kv, high_qc.block) {
+    val genesis_qc = match get_block_by_hash(kv, 0) { | Some(g) => g.justify | None => high_qc }
+    val qc = if (is_byzantine(c.id)) genesis_qc else high_qc
+    match get_block_by_hash(kv, qc.block) {
       | None => no_change(c)
       | Some(parent) =>
-        val block = create_leaf(request.hash, parent.hash, high_qc, parent.height + 1)
+        val block = create_leaf(request.hash, parent.hash, qc, parent.height + 1)
         {
-          consensus: c,
+          consensus: c.with("deviations", c.deviations + (if (qc.view < high_qc.view) 1 else 0)),
```
