# Blocksync

Blocksync is the component that lets a node obtain blocks it is missing from other nodes,
so that consensus can continue from where it got stuck.

---

## 1. The problem

Consensus refers to blocks **by hash**. A QC carries only the hash of the certified block, and a proposal
refers to its parent by hash. To act on a block, however, a node needs the block itself and its whole
chain of ancestors down to the last executed block.

A node can end up without some blocks:

- it was offline or restarted and missed part of the history;
- it lagged behind in views: a proposal arrived when the node had already moved on, and was dropped;
- a Byzantine leader sent its proposal only to some nodes — enough to form a QC (2f+1 votes),
  while up to f correct nodes never saw the block;
- it is a new node joining a running network.

Without the missing blocks the protocol stalls on that node:

| step | needs | without the block |
|---|---|---|
| voting | the parent and all ancestors up to `justify` (ancestry check) | cannot vote |
| commit | an unbroken chain from `b_exec` to the committed block | cannot commit or execute |
| proposing | the block of its own `high_qc` (the parent of the new block) | cannot propose; its view is lost |
| Case 2 | the block of the highest QC received in statuses | same as above |

In our Quint model the effect cascades: a node that misses block `B_v` never stores `B_{v+1}`
(its parent is unknown), then never stores `B_{v+2}`, and so on. The node drops out for good.
With `n = 3f + 1` and f faulty nodes, losing a single correct node is enough to make a quorum impossible,
and the whole system halts.

### Why the missing block can always be fetched

A correct node votes for a block only if it stores the block **and all its ancestors**.
A QC has 2f+1 votes, at most f of them Byzantine, so **at least f+1 correct nodes store the certified block
together with its ancestors**. Any block referenced by a QC is therefore always available from some
correct signer of that QC. Blocksync relies on exactly this property.

---

## 2. Requirements

**R1. Accepts missing-block requests.**
Blocksync receives from the consensus state machine of a correct node `p` a request `MissingBlock(h)`:
`p` needs the block with hash `h` and does not have it in its storage.

**R2. Returns missing blocks to the consensus state machine.**
Blocksync delivers blocks back to the consensus state machine of `p` as `BlockSynced(b)` inputs. A delivered block is
genuine: its hash is the one it was requested or referenced by, so `p` never receives a forged block
under a requested hash.

**R3. Eventually every missing block created by correct nodes is delivered.**
If a correct node `p` requests `MissingBlock(h)` and the block `b` with hash `h` was created by a correct
node (proposed by a correct leader), then eventually `b` is delivered to the consensus state machine of `p`.
The same holds for every block with a QC, since such a block is stored by at least f+1 correct nodes.

Blocks created only by Byzantine nodes and never certified may never be delivered: no correct node is
obliged to store them, and no correct node needs them to make progress.

R3 is a liveness property; it is required to hold after GST, when messages between correct nodes are
delivered in bounded time.

---

## 3. Interface between the consensus state machine and blocksync

The interface is deliberately minimal: consensus only knows *"I am missing the block with this hash"*
and *"here is a verified block that connects to my chain"*. Peer selection, retries, ranges and
certificates are internal to blocksync.

```
 consensus state machine (sm + driver)              blocksync
 ─────────────────────────────────────              ─────────
 NewProposal(b), parent of b not in storage
   └─▶ out: MissingBlock(b.parent)      ──────────▶  process missing block

   in:  BlockSynced(p_k), ..., BlockSynced(p_1)  ◀── delivers the chain bottom-up,
        (pushed to the front of the node queue)      p_1 = the requested block
```

> [design.md](design.md), Section 3, refines this interface: `MissingBlock` carries the QC of the missing
> block and the parked proposal, and is emitted only behind a request gate.

### Output: `SmOutput.MissingBlock(Hash)` — `sm.qnt`

```quint
type SmOutput =
| NoSmOutput
| MissingBlock(Hash)
```

`apply_state` emits `MissingBlock(parent)` when the input taken from the queue is a `NewProposal`
whose parent is not in the node's storage (`check_missing_parent`). The proposal is not passed to the
driver in that case.

Checking only the parent is enough: a block is stored only when its parent is already stored, so a
stored parent implies that all ancestors are stored as well.

### Input: `DriverInput.BlockSynced(Block)` — `driver.qnt`

```quint
type DriverInput =
  ...
| BlockSynced(Block)
```

Blocksync delivers blocks as `BlockSynced` inputs. The intended handling in the driver is:

1. `ReceivedQC(b.justify)` — the QC inside the block may raise `high_qc`;
2. store `b`;
3. `update(b)` — advance the lock and commit;
4. **no vote** — the view of a synced block is long gone.

The handling is implemented in `on_block_synced` (`driver.qnt`).

### Why hashes and not heights or views

- **Heights are not unique.** Several blocks can share a height (forks; see the tail-forking pairs in our
  traces). A height uniquely identifies a block only on the committed chain.
- **Views are not unique either.** At most one block per view gets a QC, but a Byzantine leader can send
  different blocks with the same view to different nodes. A view request may return the wrong one.
  Blocks also do not store their view; the view lives in the proposal and in the QC.
- **The hash is already known.** The requester learns about a missing block from a QC, which carries the
  block hash. The hash also makes responses self-verifying: the node checks the hash of what it received
  instead of trusting the peer.

Ranges by view or height are still useful for catching up on the committed chain, but they are an
internal strategy of blocksync, not part of the consensus interface.

### Contract for delivered blocks

- blocks are delivered **bottom-up**: the parent of each delivered block is already stored or was
  delivered just before it;
- each block's hash matches the hash it was requested or referenced by;
- the delivered chain connects to the node's local storage.

With this contract the driver never sees a block whose parent is unknown, and the invariant
"every stored block has all its ancestors stored" is preserved.

---

## 4. How it is wired in the Quint model

The demonstration client is replaced by the protocol from [design.md](design.md): `blocksync.qnt` (client
and server), the request gate and the T2 trigger in `sm.qnt`, `BlockSynced` handling in `driver.qnt`, the
sync network, faulty sync replies and invariants in `main.qnt`. See design.md, Sections 5 and 7.
