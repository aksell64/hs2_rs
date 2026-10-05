module types {
  type NodeID = int

  type Option[a] =
  | Some(a)
  | None

  type AggregatedSignature =
  | QuorumCert(Set[NodeID])
  | OneHonestCert(Set[NodeID])
  | InvalidCert

  type Certificate = {
    signature: AggregatedSignature,
  }

  type CertifiedBlock = {
    certificate: Certificate,
    block: Hash,
    view: View,
  }

  type CertifiedView = {
    certificate: Certificate,
    view: View,
  }

  type Hash = int

  type Value = Hash

  type Height = int

  type View = int

  type Block = {
    hash: Hash,
    parent: Hash,
    height: Height,
    justify: CertifiedBlock
  }

  pure def create_leaf(hash: Hash, parent: Hash, qc: CertifiedBlock, height: Height): Block = {
    {hash: hash, parent: parent, justify: qc, height: height}
  }

  type GenericVote = {
    view: View,
    block: Hash,
  }

  const committee_size: int

  pure def faulty(n: int): int = (n - 1) / 3

  pure def quorum(n: int): int =
    if (n - faulty(n) > 1) n - faulty(n) else 1

  pure val hash_variants: int = 2

  pure def block_variant_hash(view: View, proposer: NodeID, version: int): Hash =
    (view * committee_size + proposer) * hash_variants + version

  pure def block_hash(view: View, proposer: NodeID): Hash = block_variant_hash(view, proposer, 0)

  pure def timeout_quorum(n: int): int = 2 * faulty(n) + 1

  pure def is_valid_qc(qc: CertifiedBlock): bool =
    match qc.certificate.signature {
      | QuorumCert(signers) => signers.size() >= quorum(committee_size)
      | _ => false
    }

  pure def is_valid_tc(tc: CertifiedView): bool =
    match tc.certificate.signature {
      | QuorumCert(signers) => signers.size() >= timeout_quorum(committee_size)
      | _ => false
    }

  type Proposal = {
    block: Block,
    view: View,
  }
}

