module keeper {
  import types.* from "./types"

  type InputKeeper[a] = {
    by_view: View -> List[a],
    by_block: Hash -> List[a],
  }

  pure val empty_keeper: InputKeeper[a] =
    { by_view: Map(), by_block: Map() }

  pure def buffer_for_view(k: InputKeeper[a], view: View, input: a): InputKeeper[a] =
    val waiting = if (k.by_view.keys().contains(view)) k.by_view.get(view) else []
    k.with("by_view", k.by_view.put(view, waiting.append(input)))

  pure def buffer_for_block(k: InputKeeper[a], hash: Hash, input: a): InputKeeper[a] =
    val waiting = if (k.by_block.keys().contains(hash)) k.by_block.get(hash) else []
    k.with("by_block", k.by_block.put(hash, waiting.append(input)))

  pure def release_view(k: InputKeeper[a], view: View): { keeper: InputKeeper[a], inputs: List[a] } =
    val ready = k.by_view.keys().filter(v => v <= view)
    // Match BTreeMap order, preserving insertion order within each view.
    val ordered = ready.fold([], (acc, v) =>
      acc.select(w => w < v).concat([v]).concat(acc.select(w => w > v)))
    val inputs = ordered.foldl([], (acc, v) => acc.concat(k.by_view.get(v)))
    val remaining = k.by_view.keys().exclude(ready).mapBy(v => k.by_view.get(v))
    { keeper: k.with("by_view", remaining), inputs: inputs }

  pure def release_block(k: InputKeeper[a], hash: Hash): { keeper: InputKeeper[a], inputs: List[a] } =
    val inputs = if (k.by_block.keys().contains(hash)) k.by_block.get(hash) else []
    val remaining = k.by_block.keys().exclude(Set(hash)).mapBy(h => k.by_block.get(h))
    { keeper: k.with("by_block", remaining), inputs: inputs }
}
