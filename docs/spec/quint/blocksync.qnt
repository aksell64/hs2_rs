module blocksync {
  import types.* from "./types"
  import storage.* from "./storage"
  import driver.* from "./driver"

  // See docs/research/blocksync/design.md. A node fetches a block only when it holds a valid QC
  // for it and asks the signers of that QC; responses are verified and delivered bottom-up.

  // Small on purpose: chains longer than one response exercise the walk back (T3).
  pure val MAX_BLOCKS: int = 2

  type SyncMsg =
  | BlockRequest({ hash: Hash, attempt: int, max: int })
  | BlockResponse({ hash: Hash, attempt: int, blocks: List[Block] })

  type SyncPacket = {
    from: NodeID,
    to: NodeID,
    msg: SyncMsg,
  }

  type Fetch = {
    qc: CertifiedBlock,
    tried: Set[NodeID],
    peer: NodeID,
    attempt: int,
  }

  type BlockSync = {
    fetches: Hash -> Fetch,
    // verified blocks that do not connect to the node's storage yet
    buffered: Hash -> Block,
    // proposals waiting for their parent
    parked: Hash -> List[DriverInput],

    // ghost counters for witnesses
    delivered: int,
    retries: int,
    rejected: int,
  }

  pure val init_blocksync: BlockSync = {
    fetches: Map(),
    buffered: Map(),
    parked: Map(),
    delivered: 0,
    retries: 0,
    rejected: 0,
  }

  type ClientResult = {
    blocksync: BlockSync,
    packets: Set[SyncPacket],
    inputs: List[DriverInput],
  }

  pure def signers(qc: CertifiedBlock): Set[NodeID] =
    match qc.certificate.signature {
      | QuorumCert(s) => s
      | OneHonestCert(s) => s
      | InvalidCert => Set()
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

  // Deterministic rotation over the candidates, offset by the hash so that different fetches
  // start from different signers.
  pure def pick(candidates: Set[NodeID], hash: Hash, attempt: int): NodeID =
    val offset = (hash + attempt) % committee_size
    candidates.fold(-1, (best, x) =>
      if (best == -1 or (x - offset + committee_size) % committee_size < (best - offset + committee_size) % committee_size)
        x
      else
        best)

  pure def request(me: NodeID, hash: Hash, f: Fetch): SyncPacket =
    { from: me, to: f.peer, msg: BlockRequest({ hash: hash, attempt: f.attempt, max: MAX_BLOCKS }) }

  // Sends the request of `hash` to the next signer; starts a new round once every signer was tried.
  pure def next_attempt(bs: BlockSync, me: NodeID, hash: Hash, qc: CertifiedBlock, tried: Set[NodeID], attempt: int): ClientResult =
    val all_peers = signers(qc).exclude(Set(me))
    val fresh = all_peers.exclude(tried)
    val round = if (fresh.size() > 0) tried else Set()
    val candidates = if (fresh.size() > 0) fresh else all_peers
    if (candidates.size() == 0)
      { blocksync: bs.with("fetches", remove_key(bs.fetches, hash)), packets: Set(), inputs: [] }
    else
      val peer = pick(candidates, hash, attempt)
      val f = { qc: qc, tried: round.union(Set(peer)), peer: peer, attempt: attempt }
      { blocksync: bs.with("fetches", bs.fetches.put(hash, f)), packets: Set(request(me, hash, f)), inputs: [] }

  pure def park(bs: BlockSync, hash: Hash, parked: List[DriverInput]): BlockSync =
    if (parked.length() == 0)
      bs
    else
      val prev = if (bs.parked.keys().contains(hash)) bs.parked.get(hash) else []
      bs.with("parked", bs.parked.put(hash, prev.concat(parked)))

  // MissingBlock(qc): the state machine has already checked that the QC is valid.
  pure def on_missing(bs: BlockSync, me: NodeID, known: Set[Hash], qc: CertifiedBlock, parked: List[DriverInput]): ClientResult =
    val hash = qc.block
    val parked_bs = park(bs, hash, parked)
    if (known.contains(hash) or bs.fetches.keys().contains(hash) or bs.buffered.keys().contains(hash))
      { blocksync: parked_bs, packets: Set(), inputs: [] }
    else
      next_attempt(parked_bs, me, hash, qc, Set(), 0)

  pure def on_timeout(bs: BlockSync, me: NodeID, hash: Hash): ClientResult =
    val f = bs.fetches.get(hash)
    val res = next_attempt(bs, me, hash, f.qc, f.tried, f.attempt + 1)
    res.with("blocksync", res.blocksync.with("retries", res.blocksync.retries + 1))

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

  pure def on_response(bs: BlockSync, me: NodeID, known: Set[Hash], stored: Set[Hash], genuine: Set[Block], packet: SyncPacket): ClientResult =
    match packet.msg {
      | BlockResponse(resp) =>
        val expected = and {
          bs.fetches.keys().contains(resp.hash),
          bs.fetches.get(resp.hash).peer == packet.from,
          bs.fetches.get(resp.hash).attempt == resp.attempt,
        }
        if (not(expected))
          { blocksync: bs, packets: Set(), inputs: [] }
        else if (not(valid_chain(resp.hash, resp.blocks, genuine)))
          val res = on_timeout(bs, me, resp.hash)
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
            // T3: walk back, the block's own QC names the peers.
            next_attempt(stored_bs, me, low.parent, low.justify, Set(), 0)
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
          | None => Set()
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
    }

  pure def packet_attempt(p: SyncPacket): int =
    match p.msg {
      | BlockRequest(r) => r.attempt
      | BlockResponse(r) => r.attempt
    }
}
