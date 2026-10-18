# equivocate

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step

`driver.qnt`, `on_propose_request`: a Byzantine leader sends every node two blocks of its view (hash versions 0 and 1) instead of one broadcast; even nodes get version 0 first, odd nodes version 1 first.

Status: success; deviates in almost every trace.

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
```
