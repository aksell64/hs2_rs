# ask_self

expect: violation
model: n4f0v20t
invariant: request_gate_inv
step: step

`blocksync.qnt`, `committee_round`: the committee round does not exclude the node itself, so a fetch may wait for its own answer.

```diff
--- blocksync.qnt
-    rotation(0.to(committee_size - 1).exclude(Set(me)), seed)
+    rotation(0.to(committee_size - 1), seed)
```
