# silent

expect: safe
byzantine: 3
committee_size: 4
max_view: 20
invariant: safety_inv
step: step

`sm.qnt`, `apply_state`: the Byzantine node handles everything but sends nothing (no votes, proposals, wishes, statuses, TC relays).

Status: success, deviates in every trace, also with `step_fair`; commits still happen with `step_fair`.

```diff
--- sm.qnt
-          val updated = popped.state.with("driver", res.driver).with("kv", res.kv)
+          val silenced = is_byzantine(state.node_id) and res.net.length() > 0
+          val consensus = res.driver.consensus
+          val counted = res.driver.with("consensus", consensus.with("deviations", consensus.deviations + (if (silenced) 1 else 0)))
+          val updated = popped.state.with("driver", counted).with("kv", res.kv)

--- sm.qnt
-            net: res.net.foldl(received.net, (acc, env) => send(acc, state.node_id, env.to, env.msg)),
+            net: if (is_byzantine(state.node_id)) received.net
+                 else res.net.foldl(received.net, (acc, env) => send(acc, state.node_id, env.to, env.msg)),
```
