
module counter {

  type ProcessID = str

  var counters: ProcessID -> int

  val PROCESSES = Set("p1", "p2", "p3")
  val DELTAS = 1.to(3)

  action init = all {
    counters' = PROCESSES.mapBy(_ => 0),
  }

  action increment(pid: ProcessID, delta: int): bool = all {
    counters.keys().contains(pid),
    counters' = counters.put(pid, counters.get(pid) + delta),
  }

  action decrement(pid: ProcessID, delta: int): bool = all {
    counters.keys().contains(pid),
    counters' = counters.put(pid, counters.get(pid) - delta),
  }

  action step = {
    nondet pid = oneOf(PROCESSES)
    nondet delta = oneOf(DELTAS)
    any {
      increment(pid, delta),
      decrement(pid, delta),
    }
  }


  val keysStableInv = counters.keys() == PROCESSES

  val boundedInv = counters.keys().forall(p => counters.get(p) >= -30 and counters.get(p) <= 30)

  run incrementThenDecrementTest =
    init
      .then(increment("p1", 3))
      .expect(counters.get("p1") == 3)
      .then(decrement("p1", 2))
      .expect(counters.get("p1") == 1)
      .expect(counters.get("p2") == 0 and counters.get("p3") == 0)

  run independentCountersTest =
    init
      .then(increment("p1", 2))
      .then(increment("p2", 3))
      .then(decrement("p3", 1))
      .expect(counters.get("p1") == 2 and counters.get("p2") == 3 and counters.get("p3") == -1)

  run unknownProcessTest =
    init.then(increment("p9", 1)).fail()
}
