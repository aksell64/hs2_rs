# no_direct_link_tail_fork

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step
witnesses: tail_fork_witness, direct_link_mattered_witness

`tail_fork` plus the same removal of the direct-link check as in `no_direct_link_equivocate`.

Status: success: `safety_inv` holds, `direct_link_mattered_witness` in 0 of 300 traces.

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

--- consensus.qnt
-if (b_prime.parent == b.hash
-                and new_block.justify.view
+if (new_block.justify.view

--- consensus.qnt
-commit_up_to(locked, b, kv)
+commit_up_to(locked.with("deviations", locked.deviations + (if (b_prime.parent != b.hash) 1 else 0)), b, kv)

--- main.qnt
+  val direct_link_mattered_witness = CORRECT.exists(id => nodes.get(id).driver.consensus.deviations > 0)
+
   val safety_inv = and {
```
