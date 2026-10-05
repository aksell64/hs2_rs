module storage {
  import types.* from "./types"

  type KVStorage = {
    block_by_hash: Hash -> Block,
    cert_by_view: View -> CertifiedBlock,
    committed_branch: List[Block]
  }

  pure val empty_storage: KVStorage = {
    block_by_hash: Map(),
    cert_by_view: Map(),
    committed_branch: [],
  }

  pure def get_block_by_hash(kv: KVStorage, hash: Hash): Option[Block] =
    if (kv.block_by_hash.keys().contains(hash))
      Some(kv.block_by_hash.get(hash))
    else
      None

  pure def get_cert_by_view(kv: KVStorage, view: View): Option[CertifiedBlock] =
    if (kv.cert_by_view.keys().contains(view))
      Some(kv.cert_by_view.get(view))
    else
      None

  pure def insert_block(kv: KVStorage, block: Block): KVStorage =
    kv.with("block_by_hash", kv.block_by_hash.put(block.hash, block))

  pure def insert_cert(kv: KVStorage, cert: CertifiedBlock): KVStorage =
    kv.with("cert_by_view", kv.cert_by_view.put(cert.view, cert))

  pure def commit_block(kv: KVStorage, block: Block): KVStorage =
    kv.with("committed_branch", kv.committed_branch.append(block))

  pure def get_block_branch(block: Block, kv: KVStorage, depth: int): List[Block] =
    range(0, depth).foldl({ branch: [], current: Some(block) }, (acc, _) =>
    match acc.current {
      | None => acc
      | Some(b) => {
      branch: acc.branch.append(b),
      current: get_block_by_hash(kv, b.parent),
    }
    }).branch

  pure def get_parents_branch(block: Block, kv: KVStorage, depth: int): List[Block] =
    match get_block_by_hash(kv, block.parent) {
      | None => []
      | Some(parent) => get_block_branch(parent, kv, depth)
    }

}
