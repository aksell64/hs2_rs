# no_request_gate

expect: violation
test: syncGateTest

`sm.qnt`, `check_missing_parent`: a missing parent is requested for any proposal, without checking the sender and the QC.

Status: success.

```diff
--- sm.qnt
-        val gated = and {
-          signed.sender == leader(signed.proposal.view),
-          is_valid_qc(block.justify),
-          block.parent == block.justify.block,
-          get_block_by_hash(kv, block.parent) == None,
-        }
+        val gated = get_block_by_hash(kv, block.parent) == None
```
