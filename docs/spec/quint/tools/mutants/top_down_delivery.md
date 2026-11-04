# top_down_delivery

expect: violation
model: n4f0v20t
invariant: stored_ancestry_inv
step: step

`blocksync.qnt`, `release` delivers the chain top-down; `driver.qnt`, `on_block_synced` without the parent check: a block is stored before its parent.

Status: success.

```diff
--- blocksync.qnt
-          inputs: a.inputs.append(BlockSynced(a.left.get(h))),
+          inputs: [BlockSynced(a.left.get(h))].concat(a.inputs),

--- driver.qnt
-    if (get_block_by_hash(kv, b.hash) != None or get_block_by_hash(kv, b.parent) == None)
+    if (get_block_by_hash(kv, b.hash) != None)
```
