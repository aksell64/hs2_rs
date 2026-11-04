module blocksync {
  import types.* from "./types"
  import storage.* from "./storage"
  import driver.* from "./driver"
  import pacemaker.* from "./pacemaker"

  // See docs/research/blocksync/design.md. A node fetches a block only when it holds a valid QC
  // for it. It asks the author of the block first, then peers whose status reaches the block's
  // height, then the whole committee; responses are verified and delivered bottom-up.

  // Small on purpose: chains longer than one response exercise the walk back (T3).
  pure val MAX_BLOCKS: int = 2

  type SyncMsg =
  | BlockRequest({ hash: Hash, attempt: int, max: int })
  | BlockResponse({ hash: Hash, attempt: int, blocks: List[Block] })
  | NotFound({ hash: Hash, attempt: int })

  type SyncPacket = {
    from: NodeID,
    to: NodeID,
    msg: SyncMsg,
  }

  type Fetch = {
    qc: CertifiedBlock,
    // height of the missing block, exact or a lower bound (T2)
    height: Height,
    // peers still to ask in the current round
    queue: List[NodeID],
    // 0: the author and status candidates; from 1 on: the whole committee
    round: int,
    peer: NodeID,
    attempt: int,
  }

  type BlockSync = {
    fetches: Hash -> Fetch,
    // verified blocks that do not connect to the node's storage yet
    buffered: Hash -> Block,
    // proposals waiting for their parent
    parked: Hash -> List[DriverInput],
    // highest height announced by each peer
    statuses: NodeID -> Height,

    // ghost counters for witnesses
    delivered: int,
    retries: int,
    rejected: int,
    not_found: int,
  }

  pure val init_blocksync: BlockSync = {
    fetches: Map(),
    buffered: Map(),
    parked: Map(),
    statuses: Map(),
    delivered: 0,
    retries: 0,
    rejected: 0,
    not_found: 0,
  }

  type ClientResult = {
    blocksync: BlockSync,
    packets: Set[SyncPacket],
    inputs: List[DriverInput],
  }

  pure def remove_key(m: a -> b, k: a): a -> b =
    m.keys().exclude(Set(k)).mapBy(x => m.get(x))

  pure def has_block(kv: KVStorage, hash: Hash): bool =
    kv.block_by_hash.keys().contains(hash)

  // Hashes the node has or is about to have: stored, or queued as BlockSynced.
  pure def known_hashes(kv: KVStorage, queue: List[DriverInput]): Set[Hash] =
    queue.foldl(kv.block_by_hash.keys(), (acc, input) => match input {
      | BlockSynced(b) => acc.union(Set(b.hash))
      | _ => acc
    })

  pure def top_height(kv: KVStorage): Height =
    kv.block_by_hash.keys().fold(0, (acc, h) => if (kv.block_by_hash.get(h).height > acc) kv.block_by_hash.get(h).height else acc)

  // `candidates` in the order of a rotation that starts at `seed`. The seed is chosen
  // nondeterministically, so the model covers every starting point; the score-weighted order of
  // the design is not modeled, it only reorders candidates.
  pure def rotation(candidates: Set[NodeID], seed: int): List[NodeID] =
    range(0, committee_size).foldl([], (acc, k) =>
      val x = (seed + k) % committee_size
      if (candidates.contains(x)) acc.append(x) else acc)

  pure def status_candidates(bs: BlockSync, me: NodeID, height: Height): Set[NodeID] =
    bs.statuses.keys().filter(p => p != me and bs.statuses.get(p) >= height)

  // Round 0: the author, then f+1 status candidates (all of them if there are fewer).
  pure def first_round(bs: BlockSync, me: NodeID, height: Height, author: NodeID, seed: int): List[NodeID] =
    val first = if (author != me) [author] else []
    val rest = rotation(status_candidates(bs, me, height).exclude(Set(author)), seed)
    val n = faulty(committee_size) + 1
    first.concat(if (rest.length() > n) rest.slice(0, n) else rest)

  pure def committee_round(me: NodeID, seed: int): List[NodeID] =
    rotation(0.to(committee_size - 1).exclude(Set(me)), seed)

  // The author of a missing block: the sender of the parked proposal (T1) or the leader of the
  // view of its QC (T2, T3).
  pure def author_of(qc: CertifiedBlock, parked: List[DriverInput]): NodeID =
    if (parked.length() > 0)
      match parked[0] {
        | NewProposal(signed) => signed.sender
        | _ => leader(qc.view)
      }
    else
      leader(qc.view)

  pure def request(me: NodeID, hash: Hash, f: Fetch): SyncPacket =
    { from: me, to: f.peer, msg: BlockRequest({ hash: hash, attempt: f.attempt, max: MAX_BLOCKS }) }

  // Sends the request of `hash` to the next peer of the queue; an empty queue starts a committee round.
  pure def send_next(bs: BlockSync, me: NodeID, hash: Hash, qc: CertifiedBlock, height: Height, queue: List[NodeID], round: int, attempt: int, seed: int): ClientResult =
    val refill = queue.length() == 0
    val q = if (refill) committee_round(me, seed) else queue
    if (q.length() == 0)
      { blocksync: bs.with("fetches", remove_key(bs.fetches, hash)), packets: Set(), inputs: [] }
    else
      val f = { qc: qc, height: height, queue: q.tail(), round: if (refill) round + 1 else round, peer: q.head(), attempt: attempt }
      { blocksync: bs.with("fetches", bs.fetches.put(hash, f)), packets: Set(request(me, hash, f)), inputs: [] }

  pure def start_fetch(bs: BlockSync, me: NodeID, hash: Hash, qc: CertifiedBlock, height: Height, author: NodeID, seed: int): ClientResult =
    send_next(bs, me, hash, qc, height, first_round(bs, me, height, author, seed), 0, 0, seed)

  pure def park(bs: BlockSync, hash: Hash, parked: List[DriverInput]): BlockSync =
    if (parked.length() == 0)
      bs
    else
      val prev = if (bs.parked.keys().contains(hash)) bs.parked.get(hash) else []
      bs.with("parked", bs.parked.put(hash, prev.concat(parked)))

  // MissingBlock(qc): the state machine has already checked that the QC is valid.
  pure def on_missing(bs: BlockSync, me: NodeID, known: Set[Hash], qc: CertifiedBlock, height: Height, parked: List[DriverInput], seed: int): ClientResult =
    val hash = qc.block
    val parked_bs = park(bs, hash, parked)
    if (known.contains(hash) or bs.fetches.keys().contains(hash) or bs.buffered.keys().contains(hash))
      { blocksync: parked_bs, packets: Set(), inputs: [] }
    else
      start_fetch(parked_bs, me, hash, qc, height, author_of(qc, parked), seed)

  pure def on_timeout(bs: BlockSync, me: NodeID, hash: Hash, seed: int): ClientResult =
    val f = bs.fetches.get(hash)
    val res = send_next(bs, me, hash, f.qc, f.height, f.queue, f.round, f.attempt + 1, seed)
    res.with("blocksync", res.blocksync.with("retries", res.blocksync.retries + 1))

  pure def on_sync_status(bs: BlockSync, from: NodeID, height: Height): BlockSync =
    val prev = if (bs.statuses.keys().contains(from)) bs.statuses.get(from) else -1
    bs.with("statuses", bs.statuses.put(from, if (height > prev) height else prev))

  // A response for `hash` is a chain b_0, b_1, ... going down from the requested block.
  // `genuine` stands for hash integrity: every block is the one its hash commits to.
  pure def valid_chain(hash: Hash, blocks: List[Block], genuine: Set[Block]): bool =
    and {
      blocks.length() >= 1,
      blocks.length() <= MAX_BLOCKS,
      blocks[0].hash == hash,
      blocks.indices().forall(i => and {
        genuine.contains(blocks[i]),
        // genesis has no parent QC
        blocks[i].height == 0 or (is_valid_qc(blocks[i].justify) and blocks[i].justify.block == blocks[i].parent),
        i == 0 or (blocks[i].hash == blocks[i - 1].parent and blocks[i].height == blocks[i - 1].height - 1),
      }),
    }

  // Moves every buffered block whose ancestors are known to the output, bottom-up, followed by
  // the proposals parked on hashes that are stored or released right now. A proposal is never
  // released ahead of a BlockSynced of its parent that is still in the queue.
  pure def release(bs: BlockSync, known: Set[Hash], stored: Set[Hash]): ClientResult =
    val top = bs.buffered.keys().fold(0, (acc, h) => if (bs.buffered.get(h).height > acc) bs.buffered.get(h).height else acc)
    val chain = range(0, top + 1).foldl({ known: known, inputs: [], left: bs.buffered }, (acc, height) =>
      acc.left.keys()
        .filter(h => acc.left.get(h).height == height and acc.known.contains(acc.left.get(h).parent))
        .fold(acc, (a, h) => {
          known: a.known.union(Set(h)),
          inputs: a.inputs.append(BlockSynced(a.left.get(h))),
          left: remove_key(a.left, h),
        }))
    val released = chain.known.exclude(known)
    val ready = bs.parked.keys().filter(h => stored.contains(h) or released.contains(h))
    val parked_inputs = ready.fold([], (acc, h) => acc.concat(bs.parked.get(h)))
    {
      blocksync: bs
        .with("buffered", chain.left)
        .with("parked", bs.parked.keys().exclude(ready).mapBy(h => bs.parked.get(h)))
        .with("fetches", bs.fetches.keys().exclude(chain.known).mapBy(h => bs.fetches.get(h)))
        .with("delivered", bs.delivered + chain.inputs.length()),
      packets: Set(),
      inputs: chain.inputs.concat(parked_inputs),
    }

  pure def expected(bs: BlockSync, from: NodeID, hash: Hash, attempt: int): bool =
    and {
      bs.fetches.keys().contains(hash),
      bs.fetches.get(hash).peer == from,
      bs.fetches.get(hash).attempt == attempt,
    }

  pure def on_response(bs: BlockSync, me: NodeID, known: Set[Hash], stored: Set[Hash], genuine: Set[Block], packet: SyncPacket, seed: int): ClientResult =
    match packet.msg {
      | NotFound(nf) =>
        if (not(expected(bs, packet.from, nf.hash, nf.attempt)))
          { blocksync: bs, packets: Set(), inputs: [] }
        else
          // No timeout to wait for: the next peer is asked at once.
          val res = on_timeout(bs, me, nf.hash, seed)
          res.with("blocksync", res.blocksync.with("not_found", res.blocksync.not_found + 1))
      | BlockResponse(resp) =>
        if (not(expected(bs, packet.from, resp.hash, resp.attempt)))
          { blocksync: bs, packets: Set(), inputs: [] }
        else if (not(valid_chain(resp.hash, resp.blocks, genuine)))
          val res = on_timeout(bs, me, resp.hash, seed)
          res.with("blocksync", res.blocksync.with("rejected", res.blocksync.rejected + 1))
        else
          // The response may reach blocks the node already has (down to genesis): keep only new ones.
          val fresh = resp.blocks.select(b => not(known.contains(b.hash)))
          val buffered = fresh.foldl(bs.buffered, (acc, b) => acc.put(b.hash, b))
          val stored_bs = bs.with("buffered", buffered).with("fetches", remove_key(bs.fetches, resp.hash))
          val low = if (fresh.length() > 0) fresh[fresh.length() - 1] else resp.blocks[0]
          if (fresh.length() == 0 or known.contains(low.parent) or buffered.keys().contains(low.parent))
            release(stored_bs, known, stored)
          else
            // T3: walk back; the block's own QC certifies its parent.
            start_fetch(stored_bs, me, low.parent, low.justify, low.height - 1, leader(low.justify.view), seed)
      | _ => { blocksync: bs, packets: Set(), inputs: [] }
    }

  // After every step of the node: drop fetches for blocks that arrived by other means and
  // release what became deliverable.
  pure def tidy(bs: BlockSync, known: Set[Hash], stored: Set[Hash]): ClientResult =
    release(bs, known, stored)

  pure def serve(kv: KVStorage, me: NodeID, packet: SyncPacket): Set[SyncPacket] =
    match packet.msg {
      | BlockRequest(req) =>
        match get_block_by_hash(kv, req.hash) {
          | Some(b) =>
            val max = if (req.max < MAX_BLOCKS) req.max else MAX_BLOCKS
            Set({ from: me, to: packet.from, msg: BlockResponse({ hash: req.hash, attempt: req.attempt, blocks: get_block_branch(b, kv, max) }) })
          | None => Set({ from: me, to: packet.from, msg: NotFound({ hash: req.hash, attempt: req.attempt }) })
        }
      | _ => Set()
    }

  pure def is_request(p: SyncPacket): bool =
    match p.msg {
      | BlockRequest(_) => true
      | _ => false
    }

  pure def is_response(p: SyncPacket): bool = not(is_request(p))

  pure def packet_hash(p: SyncPacket): Hash =
    match p.msg {
      | BlockRequest(r) => r.hash
      | BlockResponse(r) => r.hash
      | NotFound(r) => r.hash
    }

  pure def packet_attempt(p: SyncPacket): int =
    match p.msg {
      | BlockRequest(r) => r.attempt
      | BlockResponse(r) => r.attempt
      | NotFound(r) => r.attempt
    }
}
