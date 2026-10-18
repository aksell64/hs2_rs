# lock_any

expect: violation
test: lockNeverDecreasesTest

`consensus.qnt`, `update`: the lock moves to any justify, even a lower one.

Status: success; also caught by simulation with `lock_monotonic_inv`.

```diff
--- consensus.qnt
-if (new_block.justify.view > c.qc_lock.view)
+if (true)
```
