# forging_server

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step
witnesses: sync_reject_witness

`main.qnt`, `sync_serve`: the Byzantine node answers every sync request with a forged block (the requested hash on top of genesis).

Status: success; deviates in 69 of 300 traces, `sync_reject_witness` in 53 of 300.

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
