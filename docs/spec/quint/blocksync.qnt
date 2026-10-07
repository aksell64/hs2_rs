module blocksync {
  import types.* from "./types"
  import storage.* from "./storage"
  import driver.* from "./driver"

  type BlockSync = {
    requested: Set[Hash],
  }

  pure val init_blocksync: BlockSync = { requested: Set() }

  pure def request_block(bs: BlockSync, hash: Hash): BlockSync =
    bs.with("requested", bs.requested.union(Set(hash)))

  pure def has_block(kv: KVStorage, hash: Hash): bool =
    kv.block_by_hash.keys().contains(hash)

  // Demonstration only: `source` is the union of all correct nodes' storages, so a block
  // is "fetched" as soon as any correct node has it. This models the problem blocksync
  // solves (a QC guarantees some correct node stores the block), not a real protocol:
  // there are no requests, responses, peer selection or Byzantine replies.
  pure def missing_chain(source: Hash -> Block, kv: KVStorage, hash: Hash): List[Block] =
    if (not(source.keys().contains(hash)))
      []
    else
      range(0, source.get(hash).height + 1).foldl({ chain: [], cur: Some(hash) }, (acc, _) =>
        match acc.cur {
          | Some(h) =>
            if (has_block(kv, h) or not(source.keys().contains(h)))
              { chain: acc.chain, cur: None }
            else
              { chain: [source.get(h)].concat(acc.chain), cur: Some(source.get(h).parent) }
          | None => acc
        }
      ).chain

  type SyncResult = {
    blocksync: BlockSync,
    inputs: List[DriverInput],
  }

  pure def serve_requests(bs: BlockSync, kv: KVStorage, source: Hash -> Block): SyncResult =
    bs.requested.fold({ blocksync: bs, inputs: [] }, (acc, hash) =>
      val chain = missing_chain(source, kv, hash)
      if (has_block(kv, hash) or chain.length() > 0)
        {
          blocksync: acc.blocksync.with("requested", acc.blocksync.requested.exclude(Set(hash))),
          inputs: chain.foldl(acc.inputs, (inputs, b) => inputs.append(BlockSynced(b))),
        }
      else
        acc
    )
}
