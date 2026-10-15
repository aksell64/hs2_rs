# no_lock_check

expect: violation
test: voteRespectsLockTest

`consensus.qnt`, `can_vote`: the vote rule drops the lock check, a replica votes for any block of its view.

Status: success; simulation with `lock_protection_inv` does not catch it: the faulty node rarely proposes on an old QC and a fork QC needs a quorum.

```diff
--- consensus.qnt
-c.voted_view < view and block.justify.view >= c.qc_lock.view
+c.voted_view < view
```
