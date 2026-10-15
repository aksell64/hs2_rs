# equivocate_and_double_vote

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step

`equivocate` plus `consensus.qnt`, `can_vote`: the Byzantine node ignores its `voted_view` and votes for both blocks.

Status: success.

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
-c.voted_view < view and block.justify.view >= c.qc_lock.view
+(c.voted_view < view or is_byzantine(c.id)) and block.justify.view >= c.qc_lock.view

--- consensus.qnt
-consensus: c.with("voted_view", cur_view),
+consensus: c.with("voted_view", cur_view)
+          .with("deviations", c.deviations + (if (c.voted_view >= cur_view) 1 else 0)),
```
