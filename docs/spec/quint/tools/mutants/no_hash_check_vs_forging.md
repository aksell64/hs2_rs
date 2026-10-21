# no_hash_check_vs_forging

expect: violation
byzantine: 3
committee_size: 4
max_view: 20
invariant: stored_genuine_inv
step: step

`forging_server` plus `blocksync.qnt`, `valid_chain` without the hash integrity check (`genuine.contains(blocks[i])`): a forged block reaches storage.

Status: success; deviates in 17 of 300 traces.

```diff
--- blocksync.qnt
-        genuine.contains(blocks[i]),
```

```diff
--- main.qnt
     nondet p = requests_to(id).oneOf()
+    val forged = { hash: packet_hash(p), parent: 0, height: 1, justify: GENESIS_QC }
+    val forging = is_byzantine(id) and not(minted.contains(forged))
+    val consensus = nodes.get(id).driver.consensus
     all {
-      nodes' = nodes,
+      nodes' = if (forging) nodes.put(id, nodes.get(id).with("driver", nodes.get(id).driver.with("consensus", consensus.with("deviations", consensus.deviations + 1)))) else nodes,

--- main.qnt
-      sync_net' = sync_net.exclude(Set(p)).union(serve(nodes.get(id).kv, id, p)),
+      sync_net' = sync_net.exclude(Set(p)).union(if (forging) Set({ from: id, to: p.from, msg: BlockResponse({ hash: packet_hash(p), attempt: packet_attempt(p), blocks: [forged] }) }) else serve(nodes.get(id).kv, id, p)),
```
