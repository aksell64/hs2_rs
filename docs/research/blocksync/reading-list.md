# Blocksync: reading list

The reading plan is organized around three questions:

1. What blocksync is and what problem it solves.
2. What approaches exist, how they differ, their pros and cons.
3. What good quick solutions exist.

The recommended reading order is at the end of the file.

---

## 1. What it is and what problem it solves

It is better to start with practical explanations rather than papers.

- [ ] **[CometBFT — Block Sync](https://docs.cometbft.com/main/explanation/core/block-sync)** (documentation).
  Why a node needs a catch-up mode, how it decides it has caught up (its height is not below the maximum
  reported by peers), and how it switches back to consensus.

- [ ] **Aptos — [Node Networking and State Synchronization](https://aptos.dev/network/blockchain/node-networks-sync)**
  and **[State Synchronization](https://aptos.dev/network/nodes/configure/state-sync)**.
  A node synchronizes **either through consensus or through state sync**; initial bootstrapping and
  continuous syncing are separated. Aptos descends from Diem/LibraBFT, i.e. the HotStuff family.

- [ ] **Malachite ValueSync README** —
  [`specs/synchronization/valuesync/README.md`](https://github.com/circlefin/malachite/tree/main/specs/synchronization/valuesync).
  The best short explanation of the problem: consensus alone does not survive long-running operation
  if lagging nodes cannot catch up — at some point there will be no quorum left at the top of the chain.

- [ ] **[LibraBFT](https://diem-developers-components.netlify.app/papers/diem-consensus-state-machine-replication-in-the-diem-blockchain/2020-05-26.pdf)**,
  the "Omitted, gory details" paragraph. A typical move in consensus papers: synchronization is pushed into the
  assumption that "the transport retrieves block ancestors". Explains why blocksync has to be designed separately.

---

## 2. Approaches: differences, pros and cons

### A. Fetch by reference (pull by hash)

A block referenced by a message is missing — the node requests it and its ancestors.

- [ ] **[Ethereum p2p-interface: `BeaconBlocksByRoot`](https://ethereum.github.io/consensus-specs/specs/capella/p2p-interface/)** —
  the exact format of a "give me blocks by hashes" request, with limits.
- Block retrieval in Diem/Aptos works the same way: "give me N blocks ending at X".

| ➕ | ➖ |
|---|---|
| simple; responses are verified by hash; works well when lagging by 1–10 blocks | slow when far behind (the chain is walked back one step at a time); easy to spam without limits |

### B. Separate catch-up mode

A lagging node leaves consensus, downloads committed blocks in batches together with commit proofs, then returns.

- [ ] **[CometBFT blocksync](https://pkg.go.dev/github.com/cometbft/cometbft/blocksync)** — implementation.
  Be sure to look at [`BlockPool.IsCaughtUp`](https://github.com/cometbft/cometbft/blob/v0.40.x/blocksync/pool.go#L232)
  and `sync_tracker.go` — how "is it time to switch to consensus" is decided (notes at the end of the file).
- [ ] **Fastsync: [Formal Specification and Model Checking of the Tendermint Blockchain Synchronization Protocol](https://drops.dagstuhl.de/entities/document/10.4230/OASIcs.FMBC.2020.10)**
  ([extended version](https://link.springer.com/chapter/10.1007/978-3-030-61362-4_27),
  [English + TLA+ specifications](https://github.com/tendermint/spec/tree/master/rust-spec/fastsync)).
  The model checker found a violation of synchronization termination in the protocol.
- [ ] **[Ethereum: `BeaconBlocksByRange`](https://ethereum.github.io/consensus-specs/specs/capella/p2p-interface/)** — range requests.

| ➕ | ➖ |
|---|---|
| fast when far behind; batches; parallel downloads from different peers | complex mode-switching conditions: a node may decide it has caught up while the network has already moved on |

### C. Synchronization in parallel with consensus, without modes

- [ ] **Malachite ValueSync** — README and Quint model:
  `valuesyncClient.qnt`, `valuesyncServer.qnt`, `vsyncWithConsensus.qnt`, `quint/README.md` (invariants).

| ➕ | ➖ |
|---|---|
| consensus is the only place where decisions are made; no switching; well suited for formal verification | slower than a separate mode when hugely behind, unless requests are parallelized |

### D. State transfer (state sync, snapshots)

Instead of downloading the whole history, take a state snapshot with a proof.

- [ ] **[Aptos state sync](https://aptos.dev/network/nodes/configure/state-sync)** — fast sync via snapshots, waypoints as checkpoints.
- [ ] **PBFT** (Castro, Liskov, TOCS 2002), the state transfer section — checkpoints and a partition tree of the state.
- [ ] **[BFT-SMaRt: Collaborative State Transfer](https://www.usenix.org/system/files/conference/atc13/atc13-bessani.pdf)** —
  several replicas serve the state in parallel.

| ➕ | ➖ |
|---|---|
| the only option for a huge history | complex: requires snapshots with a proof (a state root under a commit certificate); too early for us |

### E. Data availability separate from consensus

- [ ] **[Narwhal and Tusk](https://sonnino.com/papers/narwhal-and-tusk.pdf)** — blocks are disseminated by a separate layer,
  2f+1 nodes sign "we store this block", and consensus works with hashes only.
- [ ] **[Beluga: Block Synchronization for BFT Consensus Protocols](https://arxiv.org/abs/2511.15517)** (2025) —
  resource-aware synchronization, the **pull induction** attack, an implicit proof of availability
  (f+1 references to a block), theorems for partial synchrony. Used in Sui.

| ➕ | ➖ |
|---|---|
| strong availability guarantees, protection against resource attacks | an architectural rework rather than an add-on |

---

## 3. Good quick solutions

For a HotStuff-like protocol such as ours, the fastest way to a working design is:

1. **Skeleton — Malachite ValueSync** (approach C).
   Status `{base, top}`, request, response, retry with another peer on timeout, the response is fed into consensus
   as a regular input. Fits our driver and `Replay`; their Quint model is a direct template for ours. **Read first.**
2. **Message format — Ethereum `ByRoot` and `ByRange`.**
   By hash — to fetch ancestors, by range — to catch up, with strict size limits.
3. **HotStuff specifics — LibraBFT/Aptos.**
   `high_qc` in every vote and timeout to detect lagging immediately; request from the QC signers
   (QC = 2f+1 votes ⇒ at least f+1 correct nodes store the block with all its ancestors).
4. **What not to do — Beluga and Fastsync.**
   Do not download everything from everyone (pull induction); be careful with "caught up / not caught up" conditions.

---

## Reading order

1. [ ] CometBFT doc — Block Sync
2. [ ] Malachite ValueSync README
3. [ ] Malachite `valuesyncClient.qnt` (+ `vsyncWithConsensus.qnt`)
4. [ ] Ethereum `BeaconBlocksByRoot` / `BeaconBlocksByRange`
5. [ ] CometBFT `BlockPool.IsCaughtUp` + `sync_tracker.go`
6. [ ] Fastsync (FMBC 2020 paper)
7. [ ] Beluga
8. [ ] The rest as needed: Aptos state sync, Narwhal, PBFT state transfer, BFT-SMaRt CST

---

## Notes: how CometBFT decides "have I caught up"

[`BlockPool.IsCaughtUp`](https://github.com/cometbft/cometbft/blob/v0.40.x/blocksync/pool.go#L232) is the only condition
for leaving blocksync and switching to consensus (the reactor checks it once per second).

1. **No peers → not caught up.** Without peers the height of the network is unknown.
2. **Main condition:** `pool.height >= maxPeerHeight - 1`.
   `-1` because block H is verified by the commit stored in block H+1 (`LastCommit`): the peers' top block cannot
   be verified through blocksync, so the node gets it in consensus.
3. **Fallback against Byzantine peers.** A peer can claim a huge height and serve nothing — then the main condition
   never holds. So 3 s after start the decision is made by `SyncTracker`, which looks only at what cannot be forged:
   local clock and timestamps in the headers of verified blocks. If blocks arrive no faster than the network produces
   them, we are no longer catching up and it is time to switch to consensus.
   In addition, `updateMaxPeerHeight` ignores peers that claim data they cannot serve.

Takeaways:

- leaving too early is safe (consensus will catch up on its own); getting stuck forever is a loss of liveness;
- "have I caught up" cannot be decided precisely locally, so time-based heuristics are used, which are hard to verify formally;
- exactly because of this problem Malachite gave up the separate mode and runs synchronization in parallel with consensus.
