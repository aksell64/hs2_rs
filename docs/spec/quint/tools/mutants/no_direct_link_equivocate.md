# no_direct_link_equivocate

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step
witnesses: direct_link_mattered_witness

`equivocate` plus `consensus.qnt`, `update`: the commit rule drops the direct-link check `b'.parent = b` on every node; the commits it would have blocked are counted in `deviations` of correct nodes (`direct_link_mattered_witness`).

Status: success: `safety_inv` holds, `direct_link_mattered_witness` in 0 of 300 traces — with `justify` always on the direct parent the check is redundant.

```diff
--- driver.qnt
       | Propose(proposal) =>
         val result = driver_no_change(proposed.driver, kv)
-        add_net_msg(All, ProposalMsg(proposal), result)
+        if (is_byzantine(d.pacemaker.id))
+          val other = proposal.with("block", proposal.block.with("hash", block_variant_hash(proposal.view, d.pacemaker.id, 1)))
+          val consensus = result.driver.consensus
+          val counted = result.with("driver", result.driver.with("consensus", consensus.with("deviations", consensus.deviations + 1)))
+          0.to(committee_size - 1).fold(counted, (r, n) =>
+            val first = if (n % 2 == 0) proposal else other
+            val second = if (n % 2 == 0) other else proposal
+            add_net_msg(Node(n), ProposalMsg(second), add_net_msg(Node(n), ProposalMsg(first), r)))
+        else
+          add_net_msg(All, ProposalMsg(proposal), result)

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
