# Blocksync: design (MVP)

This document describes the blocksync algorithm we are going to specify in Quint and later implement.
The problem statement, requirements R1–R3 and the consensus interface are in [blocksync.md](blocksync.md);
the survey of existing approaches is in [reading-list.md](reading-list.md).

**Summary.** Blocksync is a pull-by-hash protocol that runs in parallel with consensus (no separate
catch-up mode, as in Malachite ValueSync). A node requests a block only when it holds a valid QC for it.
A QC carries one threshold signature and does not reveal its signers, so peers come from status messages:
every node periodically announces the height of its highest stored block, and the client asks, one at a time,
first the author of the block, then f+1 peers that announced at least the needed height (at least one of
them is correct). Responses are verified
by hash and delivered to the driver bottom-up as regular inputs.

---

## 1. Scope

In scope (MVP):

- fetching certified blocks that a node is missing, together with their ancestors, by hash;
- Byzantine servers: silent, forged blocks, truncated or unrelated chains, unsolicited responses;
- handling of synced blocks in the driver.

Out of scope (phase 2, Section 9):

- catching up a node that is far behind (thousands of blocks) by committed-height ranges;
- pruned storage and state sync;
- rate limiting beyond the per-peer limits in Section 6.

---

## 2. Key observation: every needed block comes with a QC

A valid block is built directly on its QC (`extends_justify`, `consensus.qnt`):

```quint
block.parent == block.justify.block
```

Every way a node learns about a block it does not have goes through a QC:

| trigger | missing block | QC at hand |
|---|---|---|
| T1. proposal whose parent is not stored | `proposal.block.parent` | `proposal.block.justify` (same block) |
| T2. a new `high_qc` whose block is not stored (Case 2 statuses, QC from a proposal of a later view) | `high_qc.block` | `high_qc` |
| T3. ancestors of a fetched block | `b.parent` | `b.justify` (same block) |

A QC is backed by 2f+1 votes, at most f of them Byzantine. A correct node votes only for a block it stores
together with all its ancestors. Hence:

- **availability:** at least f+1 correct nodes store the block and all its ancestors;
- **no pull induction:** a QC cannot be forged (2f+1 signature shares), so a Byzantine node cannot make a
  correct node chase a hash that no correct node has. Without a valid QC, nothing is requested.

The QC does not tell *which* nodes these are: it holds one threshold (TBLS) signature recovered from any 2f+1
shares, with no signer bitmap. Peer selection therefore uses status messages (Section 4.6).

As a consequence, **blocksync only ever delivers certified blocks** (and genesis). Blocks created by a
Byzantine leader and never certified are not fetched; this matches R3.

---

## 3. Interface changes

### 3.1. `MissingBlock` carries the QC

```quint
type SmOutput =
| NoSmOutput
| MissingBlock({ qc: CertifiedBlock, height: Height, parked: List[DriverInput] })   // was MissingBlock(Hash)
```

The QC gives the hash (`qc.block`) and proves that the block exists. `height` is the height of the missing
block, used to pick peers (Section 4.6): `proposal.block.height - 1` for T1, `low.height - 1` for T3. For T2
the height is unknown and the client uses a lower bound: its highest stored height plus one. `parked`
carries the proposal that waits for the block (T1), empty for T2.

### 3.2. When the state machine emits `MissingBlock`

`MissingBlock(qc)` is emitted only if **all** hold (the request gate):

1. `is_valid_qc(qc)`;
2. `qc.block` is not in the node's storage;
3. for T1: the sender is the leader of `proposal.view` and `proposal.block.parent == proposal.block.justify.block`.

The current model checks the parent of every `NewProposal` before the driver checks the sender and the QC
(`check_missing_parent` in `sm.qnt`), so a non-leader can trigger a request for an arbitrary hash. The gate
above closes that.

T2 is new: after each driver step the state machine checks whether the block of `high_qc` is stored.
Today a Case 2 leader that learns a higher QC from a status, but does not have its block, silently fails to
propose (`on_propose` returns `no_change`) and its view is lost.

### 3.3. Parked proposals

Today a proposal with a missing parent is dropped. With blocksync it is **parked** in the client under the
missing hash and pushed back to the queue right after the chain that completes it. If the node has moved on
to a later view in the meantime, the driver ignores the proposal as usual; if the block got a QC, it will be
fetched later through T1/T2.

### 3.4. `BlockSynced` handling in the driver

`BlockSynced(b)`:

1. if `b.hash` is already stored: no change (late or duplicate response);
2. if `b.parent` is not stored: no change (the contract in 4.4 is violated; defensive check);
3. `ReceivedQC(b.justify)` in the pacemaker — may raise `high_qc` and advance the view, exactly as for a QC
   received in a proposal;
4. `call_update_locked(b)` — store the block, advance the lock, commit;
5. **no vote**: the view of a synced block is in the past.

---

## 4. Protocol

### 4.1. Messages

Blocksync messages go through their own channel, separate from consensus messages:

```quint
type SyncMessage =
| BlockRequest({ id: RequestID, hash: Hash, max: int })
| BlockResponse({ id: RequestID, blocks: List[Block] })   // blocks[0].hash == hash, then ancestors
| NotFound({ id: RequestID })                              // the server does not store hash
| SyncStatus({ height: Height })                           // height of the highest stored block
| StatusRequest                                            // ask for a SyncStatus now
```

`SyncStatus` is a separate message from the pacemaker's `StatusMsg`. A request names the block by hash, not
by height: on the uncommitted tail two certified blocks can share a height.

`max` caps the number of blocks in a response (`MAX_BLOCKS`, a model constant). `RequestID` is
`(client, hash, attempt)`; it lets the client match a response to the request it actually sent.

### 4.2. Client state

```quint
type Fetch = {
  qc: CertifiedBlock,            // why we fetch: the hash and the proof that the block exists
  height: Height,                // height of the missing block (exact or lower bound)
  queue: List[NodeID],           // peers still to ask in the current round (Section 4.6)
  peer: Option[NodeID],          // the peer we are waiting for
  attempt: int,
  parked: List[DriverInput],     // proposals waiting for this block
}

type SyncClient = {
  fetches: Hash -> Fetch,        // one entry per missing hash
  buffered: Hash -> Block,       // verified blocks that do not connect to storage yet
  statuses: NodeID -> Status,    // last status of each peer
  scores: NodeID -> PeerScore,   // request history of each peer (Section 4.7)
}

type Status = {
  height: Height,                // highest height announced by the peer, never decreases
  received: Time,                // when the last SyncStatus from the peer arrived
}
```

### 4.3. Client algorithm

**On `MissingBlock(qc, height)`** (`h = qc.block`):
- `h` already in `fetches`: add the parked proposal, if any, and stop;
- otherwise create a `Fetch` and send a request (below).

**Send a request:** take the next `p` from `queue`; if `queue` is empty, build a new one (Section 4.6).
Send `BlockRequest{ id, h, MAX_BLOCKS }` to `p` and set `peer = Some(p)`.

**On `BlockResponse{ id, blocks }`** from `p`:
1. drop it unless `id` matches an open fetch and `p` is its `peer` (unsolicited or stale);
2. verify (4.4); on failure treat it as a timeout;
3. add the verified blocks to `buffered`;
4. let `low` be the lowest buffered block of this chain:
   - `low.parent` is stored: emit `BlockSynced` for the chain **bottom-up**, then the parked proposals,
     to the front of the node queue; drop the fetch and the buffered chain;
   - otherwise: keep the chain buffered and replace the fetch with one for `low.parent`, using
     `low.justify` (T3) and height `low.height - 1`.

**On `NotFound{ id }`** from `peer`: send the request to the next peer at once, `attempt + 1`.

**On timeout of a fetch:** send the request to the next peer (as above), `attempt + 1`.

**On `SyncStatus{ height }`** from `p`: `statuses[p] = { height: max(statuses[p].height, height), received: now }`.
Nothing else: a status is not verified, not forwarded, and changes neither consensus state nor scores.

**On every step:** drop fetches whose hash got stored by other means (e.g. the proposal arrived through
consensus); release their parked proposals.

### 4.4. Verification of a response

For `blocks = [b_0, b_1, ..., b_k]` answering a request for `h`:

- `1 <= k + 1 <= max`;
- `b_0.hash == h`, and for every `i > 0`: `b_i.hash == b_{i-1}.parent`;
- every block is well-formed: `is_valid_qc(b_i.justify)`, `b_i.justify.block == b_i.parent`,
  heights decrease by one;
- **hash integrity:** each `b_i` is the block its hash commits to.

In a real implementation integrity is `hash(content(b_i)) == b_i.hash`. In Quint a hash is an `int` field,
so the model keeps a ghost map of minted blocks and the check is `b_i == minted.get(b_i.hash)`. This encodes
the assumption that the hash function is collision-resistant.

The server may stop early (it is not obliged to send `max` blocks), so a short response is valid; the client
continues from the lowest block (T3).

### 4.5. Server algorithm

**On `BlockRequest{ id, h, max }`** from `c`:
- if `h` is stored: reply `BlockResponse{ id, [h, parent(h), ...] }` with at most `max` blocks, stopping at
  genesis;
- otherwise: reply `NotFound{ id }`, so the client moves on without waiting for a timeout.

**Statuses:** every `STATUS_INTERVAL`, and on `StatusRequest`, send `SyncStatus{ height }` of the highest
stored block to all nodes, even if the height did not change: a repeated status also tells that the
peer is alive. Because stored blocks always have their ancestors (`stored_ancestry_inv`), a
correct node with height `H` stores one complete chain up to `H`.

The server reads its storage only; it never changes consensus state.

### 4.6. Peer selection

A QC holds one TBLS signature and does not reveal its signers, so the client picks peers from statuses.
For a fetch of height `h`:

0. **The author first.** One node is known to have the block if it is correct, without any status:
   for T1 the sender of the parked proposal (the current leader built on the missing parent), for T2 and T3
   the leader of `qc.view` (it proposed the missing block). It heads the first round's queue and is not
   repeated in steps 2–3.
1. `C = { p != self : statuses[p].height >= h and now - statuses[p].received < STATUS_TTL }`. A peer
   without a fresh status (for example, one that crashed) is not a candidate; `STATUS_TTL` is a few
   `STATUS_INTERVAL`s.
2. If `|C| >= f+1`: the round queue is `f+1` peers of `C` drawn by score (4.7). At most `f` statuses are
   Byzantine, so the queue contains at least one correct node that stores a chain up to height `h` or above.
3. If `|C| < f+1` (statuses lag behind a fresh block): send `StatusRequest` to all, wait `STATUS_WAIT`,
   recompute `C`; if it is still smaller than `f+1`, the queue is `C`.
4. When the queue is exhausted without success, the next round asks the whole committee in
   score-weighted random order, including peers without a fresh status. This round guarantees termination
   even with stale or false statuses.

Requests are sent **one at a time**; the client never polls several peers for the same block in parallel,
so a missing block costs the network one response, not `f+1`.

What the guarantee covers:

- **committed blocks:** heights are unique, so a correct peer with status `>= h` stores the block: at most
  `f` failed attempts, as with signer-based selection;
- **uncommitted tail (T1, T2):** a correct peer with status `>= h` may store a different branch at height
  `h` and answers `NotFound`. Such attempts cost one round trip, not a timeout; in the worst case the client
  reaches the committee round.

Statuses only choose whom to ask. Every response is still verified (4.4), so a false status can delay a
fetch but cannot make a node store a wrong block.

### 4.7. Peer scoring

Each node keeps a score for every other committee member and uses it to order sync requests. The score
only changes *whom to ask first*; it never decides whether a response is accepted (that is 4.4).

```quint
type PeerScore = { score: float, updated: Time }
```

A peer without an entry has the initial score `0.5`. An entry not updated for `SCORE_TTL` is dropped, so
the peer starts again from `0.5`. `SCORE_TTL` is a configuration parameter and is meant to be long (hours,
not seconds): the history of a peer stays useful across many fetches.

**Update rule** (exponential moving average, as in Malachite ValueSync). After every request the node
itself sent, the score of the peer that handled it changes:

| outcome | new score | α |
|---|---|---|
| valid response (4.4), including a short one | `α·q + (1−α)·score` | `ALPHA_SUCCESS = 0.2` |
| timeout | `(1−α)·score` | `ALPHA_TIMEOUT = 0.1` |
| invalid response (fails 4.4) | `(1−α)·score` | `ALPHA_FAILURE = 0.15` |
| `NotFound` | unchanged | — |

`q` is the response quality in `[0, 1]`, computed from the response time `t`:

```
q = 1                                             if t <= SLOW_THRESHOLD
q = exp(-(t - SLOW_THRESHOLD) / SLOW_THRESHOLD)   otherwise
```

The score goes up only for valid responses, the faster the more; it goes down for timeouts and, more
steeply, for invalid responses. A slow but correct peer drifts down slowly, because a node that syncs from
it may not keep up with the network.

Why these outcomes:

- an **invalid response** is attributable to the peer: a correct node serves only blocks from its storage,
  and they always pass 4.4 (wrong hash, broken chain, invalid QC, too many blocks). This assumes an
  authenticated channel, so that nobody can send a response on behalf of another peer;
- a **short response** is not a fault: a server may stop before `max`, and the client continues with T3;
- **`NotFound`** is not a fault: a correct peer may store another branch at the requested height;
- statuses and consensus messages do not change the score.

Penalties multiply the score, so it never reaches 0: a correct peer that was down for a while is still
asked now and then, and recovers with its first valid responses.

**Use in peer selection:**

- the author (4.6, step 0) is still asked first, regardless of its score;
- the candidates of a round are drawn from `C` (or from the committee) **without replacement, with
  probability proportional to the score**: peers with a better history are asked earlier, but the order
  stays random and Byzantine peers cannot predict it;
- drawing without replacement keeps the `f+1` candidates distinct, so the guarantee of 4.6 (at least one
  correct candidate) does not depend on the scores;
- with no history all scores are `0.5` and the order is uniform random.

Compared with Malachite, there is no partial-success outcome (a short response is normal here), no
per-request exclusion list (the round queue already asks each peer once), and no reset after a minute of
inactivity (`SCORE_TTL` is long and configurable).

---

## 5. Modeling in Quint

| piece | where | what |
|---|---|---|
| `SyncMessage`, sync channel | `blocksync.qnt`, `main.qnt` | `var sync_net: Set[SyncPacket]`, `SyncPacket = { from, to, msg: SyncMsg }` (unordered, like Malachite buffers) |
| client and server | `blocksync.qnt` | pure functions `on_missing`, `on_response`, `on_timeout`, `on_sync_status`, `serve` |
| `MissingBlock(qc)`, request gate, T2, parking | `sm.qnt` | replaces `check_missing_parent` |
| `BlockSynced` | `driver.qnt` | Section 3.4 |
| ghost `minted: Set[Block]` | `main.qnt` | every block created by `on_propose` or `deliver_faulty`; used by verification and invariants |
| actions | `main.qnt` | `sync_serve`, `sync_receive`, `sync_timeout`, `sync_status`, `deliver_faulty_sync`, `deliver_faulty_status` |

**Byzantine servers.** `FAULTY` nodes do not run; `deliver_faulty_sync` picks, for an open request to a
faulty node, one of: no reply, a forged block under the requested hash, a genuine but unrelated chain, a
truncated chain, or an unsolicited response to any client.

**Fairness.** `step_fair` lets `sync_timeout` fire only when the request and its response are no longer in
flight (the peer had its chance and failed), mirroring `view_timer_ready_fair`. `step` lets it fire at any time.

The demonstration client (`serve_requests` over the union of all storages) is removed.

**Peer selection in the model.** `blocksync.qnt` follows Section 4.6: the author first, then up to `f+1`
status candidates, then committee rounds; servers answer `NotFound`. Simplifications:

- the order of candidates is a rotation from a nondeterministic seed; scores (4.7) are not modeled, since
  they only reorder candidates and the seed already covers every starting point;
- `SyncStatus` is delivered at once (`announce`); a late status equals an earlier one, because the client
  keeps the highest height. Faulty nodes announce any height (`deliver_faulty_status`);
- `STATUS_TTL`, `SCORE_TTL`, `StatusRequest` and `STATUS_WAIT` are not modeled: there is no clock, and
  without them round 0 just has fewer candidates;
- faulty nodes may also answer a false `NotFound`.

**Models.** The existing models set `|FAULTY| = f` (`n4f1v*`, `n7f2v6`), so the correct nodes are exactly a
quorum: a block that one correct node missed cannot get a QC, and blocksync is never triggered (0 requests in
100 + 100 traces of `n4f1v20` with `step_fair` and `step`). Blocksync needs models with `|FAULTY| < f` or no
faulty nodes, e.g. `n4f0v20t` (`FAULTY = Set()`) and `n7f1v12` (`FAULTY = Set(6)`); with those a request appears
within the first 100 traces under both steps.

---

## 6. Limits

- **One outstanding request per fetch**, at most `MAX_FETCHES` fetches per node. The number of fetches is
  already bounded by the number of certified blocks the node has heard of (at most one QC per view).
- **`MAX_BLOCKS` per response** on both sides: the server truncates, the client rejects longer responses.
- **Statuses:** one `SyncStatus` per node per `STATUS_INTERVAL`; the client keeps one status per peer and
  ignores statuses older than `STATUS_TTL`.
- **Scores:** kept only for committee members; entries not updated for `SCORE_TTL` are dropped.
- **Server side:** at most one request per peer in progress; further requests from the same peer wait or are
  dropped (implementation detail, not modeled).
- **Buffered chains** are bounded by the lag of the node; above `MAX_BUFFERED` the client drops the buffer and
  falls back to phase 2.

---

## 7. Properties

Implemented in `docs/spec/quint` (`main.qnt`, `scenarios_test.qnt`, `tools/mutants/`).

### Invariants (safety), part of `safety_inv` as `sync_inv`

| name | statement |
|---|---|
| `stored_ancestry_inv` | every stored block except genesis has its parent stored |
| `stored_genuine_inv` | every block stored by a correct node is in `minted` |
| `request_gate_inv` | every open fetch has a valid QC for its hash, and neither its `peer` nor its queue contains the node itself |
| `synced_certified_inv` | every queued `BlockSynced` block is certified (some QC references it) |
| existing `safety_inv` | keeps holding with blocksync enabled |

Accepting only matching responses and not voting for synced blocks are checked by tests and mutants rather
than invariants: neither is visible in the state after the step.

### Tests

| test | checks |
|---|---|
| `syncGateTest` | `MissingBlock` only for a leader's proposal with a valid QC certifying the missing parent |
| `blockSyncedStoresWithoutVoteTest` | a synced block is stored and its QC raises `high_qc`, no vote is sent; a block with an unknown parent is not stored |

### Witnesses (`quint run --witnesses`)

| name | statement |
|---|---|
| `sync_delivery_witness` | some correct node got a block from blocksync |
| `sync_retry_witness` | some fetch went to another peer (timeout, invalid response or `NotFound`) |
| `sync_reject_witness` | some response failed verification |
| `sync_not_found_witness` | some peer answered `NotFound` |

### Liveness (R3)

After GST every fetch terminates: the committee round (4.6) asks every node, at least f+1 of them are
correct and store the block, and a correct server answers. Quint simulation does not check temporal properties; R3 is
approximated by `sync_delivery_witness` under `step_fair`, and by `safety_inv` together with commits
continuing in the models where blocksync is exercised.

### Mutants (calibration)

| mutant | change | expect |
|---|---|---|
| `forging_server` (byz `n3`) | the node votes correctly but answers every sync request with a forged block | safe; `sync_reject_witness` reached |
| `no_hash_check_vs_forging` | `forging_server` plus verification without hash integrity | violation of `stored_genuine_inv` |
| `top_down_delivery` | the chain is delivered top-down and the driver does not check the parent | violation of `stored_ancestry_inv` |
| `no_request_gate` | request on any missing proposal parent, without sender and QC checks | `syncGateTest` fails |
| `vote_on_synced` | the driver votes for a synced block | `blockSyncedStoresWithoutVoteTest` fails |

Not implemented yet: a mutant for accepting unsolicited responses (a forged unsolicited response is caught
by `stored_genuine_inv` only if the hash check is also off) and a silent Byzantine server (silence of the
`FAULTY` node is already part of every model with `FAULTY`).

---

## 8. Edge cases

| case | behavior |
|---|---|
| block arrives through consensus while a fetch is open | fetch dropped on the next client step; parked proposals released |
| late response after a retry | stale `id` or peer: dropped; duplicate `BlockSynced` is a no-op in the driver |
| all f Byzantine candidates asked first | at most f timeouts before a correct candidate is asked |
| the author is Byzantine | one timeout, then the status-based candidates |
| Byzantine peers announce huge heights | they enter `C`, but at most f of them; any f+1 of `C` contain a correct node |
| correct candidate is on another branch at height `h` | `NotFound`, next peer at once |
| statuses lag behind a fresh block | `StatusRequest`; if still fewer than f+1 candidates, ask `C`, then the committee |
| synced block's QC raises `high_qc` | the node may advance its view; this is the same as a QC from a proposal |
| synced block on a dead fork | stored, never committed; harmless (storage GC is a separate topic) |
| parked proposal of a past view | driver ignores it; if the block was certified, T1/T2 fetch it later |
| node far behind (chain does not connect within `MAX_BUFFERED`) | phase 2 |
| server pruned the block | `NotFound`; client tries other peers; if all pruned, phase 2 |
| Byzantine node gossips a fake high QC | impossible to forge 2f+1 signatures; a real QC means the block exists |

---

## 9. Phase 2: catching up far behind (not modeled)

When a node is thousands of blocks behind, walking back by hash costs one round trip per `MAX_BLOCKS`
blocks. Phase 2 adds range requests for the **committed** chain, where heights are unique:

- `RangeRequest{ from, to }` → committed blocks with a commit proof for the top one (two consecutive QCs);
- the client learns the network height from QCs it receives and picks servers by `SyncStatus` (4.6);
  a Byzantine peer cannot make it chase a fake height, because the range must end in a commit proof;
- still no separate mode: range results enter the driver as `BlockSynced` like everything else.

---

## 10. Pros, cons and alternatives

**Pros**

- no mode switching: consensus stays the only place where decisions are made (as in Malachite);
- pull induction is impossible by construction: availability comes from the QC;
- self-verifying responses; statuses only pick peers, so false statuses and Byzantine servers can only delay;
- works with TBLS certificates, which hide the signers;
- peer scoring: peers that timed out, sent invalid responses or answered slowly are asked later;
- small change to consensus: one new driver input handler and the request gate in the state machine.

**Cons**

- slow when far behind (one round trip per `MAX_BLOCKS` blocks) until phase 2;
- one peer at a time per fetch: latency of up to f timeouts when Byzantine candidates are asked first;
- periodic `SyncStatus` broadcast: O(n^2) messages per `STATUS_INTERVAL`;
- on the uncommitted tail a correct candidate may be on another branch, so more attempts than for committed
  blocks;
- buffered chains cost memory proportional to the lag;
- the parked-proposal path adds state to the state machine.

**Alternatives considered**

| approach | why not now |
|---|---|
| ask the signers of the QC | a TBLS certificate does not reveal its signers |
| Malachite-style requests by height | heights are not unique on the uncommitted tail; requests stay by hash, statuses are used only to pick peers |
| separate catch-up mode (CometBFT) | mode-switching conditions are hard to get right and to verify |
| ask several peers at once | faster, but multiplies load: f+1 responses for one missing block |
| state sync / snapshots | too early: needs state roots under commit certificates |
| separate data availability layer (Narwhal) | an architectural rework |
