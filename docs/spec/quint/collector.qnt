module collector {

  import types.* from "./types"

  type CollectorInput =
  | GenericVoteInput({ sender: NodeID, vote: GenericVote })
  | WishInput({ sender: NodeID, view: View })

  type CollectorOutput =
  | GenericOutput(GenericCollectorOutput)
  | TimeoutOutput(TimeoutCollectorOutput)

  type GenericCollectorOutput =
  | Ready(CertifiedBlock)
  | AlreadyReady(View)
  | NotReady

  type TimeoutCollectorOutput =
  | TimeoutReady(CertifiedView)
  | TimeoutAlreadyReady(View)
  | TimeoutNotReady

  type Collector = {
    generic_collectors: View -> GenericCertificateCollector,
    timeout_collectors: View -> TimeoutCertificateCollector,
  }

  type CollectorResult = {
    new_collector: Collector,
    out: CollectorOutput
  }

  pure def apply_collector(collector: Collector, input: CollectorInput): CollectorResult =
    match input {
      | GenericVoteInput(signed) =>
        val vote = signed.vote
        val result = add_generic_vote(generic_collector_at(collector, vote.view), signed.sender, vote)
        {
          new_collector: collector.with("generic_collectors",
            collector.generic_collectors.put(vote.view, result.collector)),
          out: GenericOutput(result.out),
        }
      | WishInput(wish) =>
        val result = add_wish(timeout_collector_at(collector, wish.view), wish.sender, wish.view)
        {
          new_collector: collector.with("timeout_collectors",
            collector.timeout_collectors.put(wish.view, result.collector)),
          out: TimeoutOutput(result.out),
        }
    }

  pure def generic_collector_at(collector: Collector, view: View): GenericCertificateCollector =
    if (collector.generic_collectors.keys().contains(view))
      collector.generic_collectors.get(view)
    else
      { votes: Map(), ready: false }

  type GenericCertificateCollector = {
    votes: NodeID -> GenericVote,
    ready: bool
  }

  type GenericCollectorResult = {
    collector: GenericCertificateCollector,
    out: GenericCollectorOutput
  }

  pure def add_generic_vote(
      generic_collector: GenericCertificateCollector,
      sender: NodeID,
      vote: GenericVote): GenericCollectorResult =
    if (generic_collector.ready)
      to_generic_already_ready(generic_collector, vote.view)
    else if (generic_collector.votes.keys().contains(sender))
      { collector: generic_collector, out: NotReady }
    else
      val new_generic_collector =
        generic_collector.with("votes", generic_collector.votes.put(sender, vote))
      if (is_generic_ready(new_generic_collector, vote.block))
        to_generic_ready(new_generic_collector, vote.view, vote.block)
      else
        { collector: new_generic_collector, out: NotReady }

  pure def to_generic_already_ready(collector: GenericCertificateCollector, view: View): GenericCollectorResult =
    { collector: collector, out: AlreadyReady(view) }

  pure def to_generic_ready(
      collector: GenericCertificateCollector,
      view: View,
      block: Hash): GenericCollectorResult =
    {
      collector: collector.with("ready", true),
      out: Ready({
        certificate: { signature: QuorumCert(signers_of(collector, block)) },
        block: block,
        view: view,
      }),
    }

  pure def is_generic_ready(collector: GenericCertificateCollector, block: Hash): bool =
    signers_of(collector, block).size() >= quorum(committee_size)

  pure def signers_of(collector: GenericCertificateCollector, block: Hash): Set[NodeID] =
    collector.votes.keys().filter(id => collector.votes.get(id).block == block)

  type TimeoutCertificateCollector = {
    wishes: Set[NodeID],
    ready: bool
  }

  type TimeoutCollectorResult = {
    collector: TimeoutCertificateCollector,
    out: TimeoutCollectorOutput
  }

  pure def timeout_collector_at(collector: Collector, view: View): TimeoutCertificateCollector =
    if (collector.timeout_collectors.keys().contains(view))
      collector.timeout_collectors.get(view)
    else
      { wishes: Set(), ready: false }

  pure def add_wish(
      timeout_collector: TimeoutCertificateCollector,
      sender: NodeID,
      view: View): TimeoutCollectorResult =
    if (timeout_collector.ready)
      { collector: timeout_collector, out: TimeoutAlreadyReady(view) }
    else if (timeout_collector.wishes.contains(sender))
      { collector: timeout_collector, out: TimeoutNotReady }
    else
      val new_timeout_collector =
        timeout_collector.with("wishes", timeout_collector.wishes.union(Set(sender)))
      if (new_timeout_collector.wishes.size() >= timeout_quorum(committee_size))
        {
          collector: new_timeout_collector.with("ready", true),
          out: TimeoutReady({
            certificate: { signature: QuorumCert(new_timeout_collector.wishes) },
            view: view,
          }),
        }
      else
        { collector: new_timeout_collector, out: TimeoutNotReady }
}
