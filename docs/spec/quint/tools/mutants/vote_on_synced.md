# vote_on_synced

expect: violation
test: blockSyncedStoresWithoutVoteTest

`driver.qnt`, `on_block_synced` sends a vote for the synced block.

Status: success.

```diff
--- driver.qnt
-      processed.with("driver", updated.driver).with("kv", updated.kv)
+      val vote = { view: updated.driver.pacemaker.get_current_view(), block: b.hash }
+      add_net_msg(Node(leader(vote.view + 1)), VoteMsg(vote), processed.with("driver", updated.driver).with("kv", updated.kv))
```
