# Blocksync: design (MVP)

This document describes the blocksync algorithm we are going to specify in Quint and later implement.
The problem statement, requirements R1–R3 and the consensus interface are in [blocksync.md](blocksync.md);
the survey of existing approaches is in [reading-list.md](reading-list.md).

**Summary.** Blocksync is a pull-by-hash protocol that runs in parallel with consensus (no separate
catch-up mode, as in Malachite ValueSync). A node requests a block only when it holds a valid QC for it,
and it asks the signers of that QC: at least f+1 of them are correct and store the block with all its
ancestors. Responses are verified by hash and delivered to the driver bottom-up as regular inputs.

---

## 1. Scope

In scope (MVP):

- fetching certified blocks that a node is missing, together with their ancestors, by hash;
- Byzantine servers: silent, forged blocks, truncated or unrelated chains, unsolicited responses;
- handling of synced blocks in the driver.

Out of scope (phase 2, Section 9):

- catching up a node that is far behind (thousands of blocks) by committed-height ranges;
- pruned storage and state sync;
- peer scoring and rate limiting beyond the per-peer limits in Section 6.

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

A QC carries its signers (`QuorumCert(signers)`), 2f+1 of them, at most f Byzantine. A correct node votes
only for a block it stores together with all its ancestors. Hence:

- **availability:** at least f+1 signers of the QC are correct and can serve the block and all its ancestors;
- **peer selection for free:** the client knows whom to ask without status gossip;
- **no pull induction:** a QC cannot be forged (2f+1 signatures), so a Byzantine node cannot make a correct
  node chase a hash that no correct node has. Without a valid QC, nothing is requested.

As a consequence, **blocksync only ever delivers certified blocks** (and genesis). Blocks created by a
Byzantine leader and never certified are not fetched; this matches R3.

---

## 3. Interface changes

### 3.1. `MissingBlock` carries the QC

```quint
type SmOutput =
| NoSmOutput
| MissingBlock({ qc: CertifiedBlock, parked: List[DriverInput] })   // was MissingBlock(Hash)
```

The QC gives both the hash (`qc.block`) and the peers (`qc.certificate.signature`); `parked` carries the
proposal that waits for the block (T1), empty for T2.

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
```

`max` caps the number of blocks in a response (`MAX_BLOCKS`, a model constant). `RequestID` is
`(client, hash, attempt)`; it lets the client match a response to the request it actually sent.

### 4.2. Client state

```quint
type Fetch = {
  qc: CertifiedBlock,            // why we fetch: hash + signers
  tried: Set[NodeID],            // signers already asked in the current round
  peer: Option[NodeID],          // the signer we are waiting for
  attempt: int,
  parked: List[DriverInput],     // proposals waiting for this block
}

type SyncClient = {
  fetches: Hash -> Fetch,        // one entry per missing hash
  buffered: Hash -> Block,       // verified blocks that do not connect to storage yet
}
```

### 4.3. Client algorithm

**On `MissingBlock(qc)`** (`h = qc.block`):
- `h` already in `fetches`: add the parked proposal, if any, and stop;
- otherwise create a `Fetch` and send a request (below).

**Send a request:** pick `p ∈ signers(qc) \ tried \ {self}`; if empty, start a new round
(`tried = {}`). Send `BlockRequest{ id, h, MAX_BLOCKS }` to `p`, set `peer = Some(p)`, add `p` to `tried`.

**On `BlockResponse{ id, blocks }`** from `p`:
1. drop it unless `id` matches an open fetch and `p` is its `peer` (unsolicited or stale);
2. verify (4.4); on failure treat it as a timeout;
3. add the verified blocks to `buffered`;
4. let `low` be the lowest buffered block of this chain:
   - `low.parent` is stored: emit `BlockSynced` for the chain **bottom-up**, then the parked proposals,
     to the front of the node queue; drop the fetch and the buffered chain;
   - otherwise: keep the chain buffered and replace the fetch with one for `low.parent`, using
     `low.justify` (T3) — its signers are the peers for the next step.

**On timeout of a fetch:** send the request again to the next signer (as above), `attempt + 1`.

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
- otherwise: no reply (the client times out and asks another signer).

The server reads its storage only; it never changes consensus state.

---

## 5. Modeling in Quint

| piece | where | what |
|---|---|---|
| `SyncMessage`, sync channel | `blocksync.qnt`, `main.qnt` | `var sync_net: NodeID -> Set[{ from: NodeID, msg: SyncMessage }]` (unordered, like Malachite buffers) |
| client and server | `blocksync.qnt` | pure functions `on_missing`, `on_response`, `on_timeout`, `serve` |
| `MissingBlock(qc)`, request gate, T2, parking | `sm.qnt` | replaces `check_missing_parent` |
| `BlockSynced` | `driver.qnt` | Section 3.4 |
| ghost `minted: Hash -> Block` | `main.qnt` | every block created by `on_propose` or `deliver_faulty`; used by verification and invariants |
| actions | `main.qnt` | `sync_client_step`, `sync_server_step`, `sync_timeout`, `deliver_faulty_sync` |

**Byzantine servers.** `FAULTY` nodes do not run; `deliver_faulty_sync` picks, for an open request to a
faulty node, one of: no reply, a forged block under the requested hash, a genuine but unrelated chain, a
truncated chain, or an unsolicited response to any client.

**Fairness.** `step_fair` lets `sync_timeout` fire only when the request and its response are no longer in
flight (the peer had its chance and failed), mirroring `view_timer_ready_fair`. `step` lets it fire at any time.

The demonstration client (`serve_requests` over the union of all storages) is removed.

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
| `request_gate_inv` | every open fetch has a valid QC for its hash, and its `peer` is a signer of that QC other than the node |
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
| `sync_retry_witness` | some fetch timed out and went to another signer |
| `sync_reject_witness` | some response failed verification |

### Liveness (R3)

After GST every fetch terminates: each round tries every signer, at least f+1 of them are correct and store
the block, and a correct server answers. Quint simulation does not check temporal properties; R3 is
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
| all f Byzantine signers asked first | at most f timeouts before a correct signer is asked |
| synced block's QC raises `high_qc` | the node may advance its view; this is the same as a QC from a proposal |
| synced block on a dead fork | stored, never committed; harmless (storage GC is a separate topic) |
| parked proposal of a past view | driver ignores it; if the block was certified, T1/T2 fetch it later |
| node far behind (chain does not connect within `MAX_BUFFERED`) | phase 2 |
| server pruned the block | it does not answer; client tries other signers; if all pruned, phase 2 |
| Byzantine node gossips a fake high QC | impossible to forge 2f+1 signatures; a real QC means the block exists |

---

## 9. Phase 2: catching up far behind (not modeled)

When a node is thousands of blocks behind, walking back by hash costs one round trip per `MAX_BLOCKS`
blocks. Phase 2 adds range requests for the **committed** chain, where heights are unique:

- `RangeRequest{ from, to }` → committed blocks with a commit proof for the top one (two consecutive QCs);
- the client learns the network height from QCs it receives (not from peers' claims), so a Byzantine peer
  cannot make it chase a fake height;
- still no separate mode: range results enter the driver as `BlockSynced` like everything else.

---

## 10. Pros, cons and alternatives

**Pros**

- no mode switching: consensus stays the only place where decisions are made (as in Malachite);
- no status gossip and no trust in peers' claims: peers and availability come from the QC;
- pull induction is impossible by construction;
- self-verifying responses; Byzantine servers can only delay, at most f timeouts per step;
- small change to consensus: one new driver input handler and the request gate in the state machine.

**Cons**

- slow when far behind (one round trip per `MAX_BLOCKS` blocks) until phase 2;
- one peer at a time per fetch: latency of up to f timeouts when Byzantine signers are asked first;
- buffered chains cost memory proportional to the lag;
- the parked-proposal path adds state to the state machine.

**Alternatives considered**

| approach | why not now |
|---|---|
| Malachite-style status `{base, top}` + requests by height | heights are not unique on the uncommitted tail; peers' claims must be trusted; reused for phase 2 only |
| separate catch-up mode (CometBFT) | mode-switching conditions are hard to get right and to verify |
| ask all signers at once | faster, but multiplies load (n requests per missing block); a possible later optimization: ask f+1 at once |
| state sync / snapshots | too early: needs state roots under commit certificates |
| separate data availability layer (Narwhal) | an architectural rework |
