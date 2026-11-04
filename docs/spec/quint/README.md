### Make targets

The commands below are wrapped in `Makefile`; the GitHub Actions workflows `spec-tests`, `spec-mutants` and
`spec-sim` call the same targets. They never run on their own: start one from the Actions tab ("Run workflow")
and pick the branch, e.g. the branch of a pull request, or `gh workflow run spec-tests.yml --ref <branch>`.

    make test                                   # scenario tests
    make mutants                                # all mutants (MUTANT_SAMPLES, MUTANT_MAX_STEPS)
    make sim                                    # simulation with step_fair, writes trace.itf.json
    make sim MODEL=n4f1v6 MAX_STEPS=500 SAMPLES=300 SEED=0x...
    make analyze                                # trace.itf.json -> analisys.md

`sim` also takes `STEP` and `INVARIANT` (defaults `step_fair`, `safety_inv`).

### Run model

    quint run models.qnt --main=n4f1v20 --step=step_fair --invariant=safety_inv \
    --max-steps=1000 --max-samples=1 --out-itf=trace.itf.json

This runs the Quint simulator on a random trace and checks safety under a fair scheduler.
The trace is saved to `trace.itf.json` in the Informal Trace Format.

It’s better to set `--max-steps` > 500 so that committed blocks appear in sufficient numbers.

Options:

- `--main` — configuration from `models.qnt` (`n4f1v6`, `n4f1v20`, `n7f2v6`, `n4f0v20`, `n4f0v20t`, `n7f1v12`).
  With `f` faulty nodes the correct ones are exactly a quorum, so a block missed by one of them is never
  certified and blocksync is never used; use `n4f0v20t` or `n7f1v12` to exercise blocksync.
- `--step` — `step_fair` (fairness assumption: a view timer fires only after the leader of the view has proposed
  and the node has processed its messages; gives commits and progress)
  or `step` (timers fire at any time, for safety without any timing assumption).
- `--invariant` — `safety_inv` combines all safety invariants; any single invariant from `main.qnt` can be used instead.
- `--max-samples` — number of traces; use 1 to analyze a trace, 300+ to search for violations.
- `--seed=0x...` — reproduces a trace; the seed is printed at the end of every run.

### Check invariants only

When the goal is just to check the invariants and the individual transitions are not of interest,
run many traces and skip the trace output:

    quint run models.qnt --main=n4f1v20 --step=step --invariant=safety_inv \
    --max-steps=500 --max-samples=1000 --verbosity=1

- `--max-samples` — the more traces, the wider the coverage; the run stops at the first violation.
- `--verbosity=1` prints only the verdict, without dumping a trace.
- Use `--step=step` for safety without any timing assumption, `--step=step_fair` to check safety on traces with many commits.
- To see which invariant failed, pass them separately: `--invariants agreement_inv lock_protection_inv ...`.
- On a violation, rerun with the printed `--seed`, `--max-samples=1` and `--out-itf=trace.itf.json`
  to get the counterexample, then analyze it as below.

### Analyze trace

    python3 tools/analyze.py trace.itf.json analisys.md --from 1 --to 30

Renders the trace as markdown:

- **Summary** — when each new QC appeared, when nodes committed, final views and committed chains.
- **Step N** — the action taken on the step and the state of every correct node after it
  (pacemaker, consensus, pending proposals, timers, queue) plus the network; changed values are in bold.

`--from` / `--to` select the steps that are rendered in detail; the summary always covers the whole trace.

### Run tests

    quint test scenarios_test.qnt --main=scenariosTest

Runs the scenario tests from `scenarios_test.qnt`: deterministic runs where every transition is given explicitly
(`apply_step`, `inject` of a faulty message) and the state is checked with `expect`.

- `--match=happyPathTest` — run one test (a substring or a regex over the `run` names).
- `--verbosity=3` — print the steps before a failed `expect`.
- `--out-itf=out_{test}_{seq}.itf.json` — save the trace of every test.
  Variables in these traces are prefixed (`scenariosTest::main::nodes`), so `tools/analyze.py` does not read them yet.

By default `quint test` runs every `run` whose name ends with `Test`; helpers like `happy_view` are not tests.

The test module uses `n = 4` with `n0` faulty and silent, so views 1, 2 and 3 have correct leaders `n1`, `n2`, `n3`.
`safety_inv` is checked after every step.

| test | scenario | checks |
|---|---|---|
| `happyPathTest` | views 1 and 2 with correct leaders (`happy_until(3)`) | every correct node votes in each view, the next leader builds the QC and proposes |
| `proposalFromNonLeaderIsIgnoredTest` | after view 1 the faulty `n0` sends `n1` a proposal for view 2 built on the real QC(1); the leader of view 2 is `n2` | `n1` drops it without voting and without taking its QC, then votes for the real proposal of `n2` |
| `lockNeverDecreasesTest` | after view 2 `n1` gets a second proposal of `n3` for view 3 built on the genesis QC; it is buffered, replayed after the real one and rejected | the rejected block still reaches `update`, and the lock of `n1` stays at QC(2) |
| `voteRespectsLockTest` | after view 2 `n1` (locked on QC(1)) gets a second proposal of `n3` for view 3 built on the genesis QC; its view timer moves it to view 3 before it handles the real proposal | `n1` rejects the fake block (no vote), then votes for the real one and locks on QC(2) |
| `syncGateTest` | pure check of `check_missing_parent` on proposals whose parent is missing | blocksync is asked only for a leader's proposal with a valid QC that certifies the parent |
| `blockSyncedStoresWithoutVoteTest` | pure check of the driver on `BlockSynced` | the block is stored and its QC raises `high_qc`, no vote is sent; a block with an unknown parent is not stored |

Helpers: `happy_view(v)` runs the correct nodes until the leader of `v + 1` has built QC(v) and proposed;
`happy_until(v)` runs views `1, ..., v - 1`; `inject(id, packet)` puts a message of a faulty node into the queue of `id`;
`proposal(proposer, version, view, justify)` builds a proposal.

### Run mutants

    python3 tools/mutants.py

A mutant is a change of the protocol code. The script copies the spec to a temporary directory, applies the changes
and runs Quint on the copy; the spec itself is not changed. A mutant has two parameters:

- **scope** — the mutation applies to every node (`all`), or only to the nodes in `byzantine`: they run the mutated
  protocol but are excluded from the invariants.
- **expect** — `violation`: the check must find one (the invariants and tests are able to fail);
  `safe`: it must not (the protocol must tolerate up to f nodes running the mutated code).

Each mutant reports one status:

- **success** — the outcome matches `expect`;
- **broken** — it does not. For `violation`: the invariant is vacuous, the simulator does not reach the bad states,
  or the scenario does not exercise the mutated code. For `safe`: the protocol is not safe against this behaviour.

```
no_lock_check        all       voteRespectsLockTest     success   seed=0x361db8de2b4b7457
lock_any             all       lockNeverDecreasesTest   success   seed=0x553d81a7d6102af1
vote_past_lock       byz=[3]   safety_inv               success   deviated in 1 of 300 traces seed=0xffae5e2b3cfbd4f8
propose_old_qc       byz=[3]   safety_inv               success   deviated in 299 of 300 traces seed=0xc2420b7115391ede
no_lock_check_vs_old_qc byz=[3]   lock_protection_inv      success   deviated in 18 of 300 traces seed=0x4f543b88953c0326
silent               byz=[3]   safety_inv               success   deviated in 300 of 300 traces seed=0x6be724b860f36e2f
tail_fork            byz=[3]   safety_inv               success   deviated in 295 of 300 traces seed=0xa6e4bc4641eff513
                     tail_fork_witness in 276 of 300 traces
equivocate           byz=[3]   safety_inv               success   deviated in 292 of 300 traces seed=0x6bb215d4cb32197f
forging_server       byz=[3]   safety_inv               success   deviated in 69 of 300 traces seed=0xc30f779cbd4f93b3
                     sync_reject_witness in 53 of 300 traces
no_hash_check_vs_forging byz=[3]   stored_genuine_inv       success   deviated in 17 of 300 traces seed=0x3d1f96f09a3a1c74
top_down_delivery    all       stored_ancestry_inv      success   seed=0xe7a420ab6c330604
no_request_gate      all       syncGateTest             success   seed=0x73bbebe8cba81d1b
vote_on_synced       all       blockSyncedStoresWithoutVoteTest success   seed=0xe644010791426af7
```

Options:

- `no_lock_check lock_any` — run only the listed mutants (default: all).
- `--samples` / `--max-steps` — number and length of traces for simulation (default 300 / 300).
- `--seed=0x...` — reproduce a run.
- `--verbose` — print the Quint output.
- `--entries` — another directory with mutant files.

Mutants are files in `tools/mutants/`, one per mutant, `<name>.md`:

````markdown
# lock_any

expect: violation
test: lockNeverDecreasesTest

What the mutation does, and the last result.

```diff
--- consensus.qnt
-      if (new_block.justify.view > c.qc_lock.view)
+      if (true)
```
````

- `# <name>` — the first line, equal to the file name.
- Header — `key: value` lines right after the title, up to the first blank line: `expect`, `test`, or `invariant`
  and `step` (and `model` without `byzantine`), `byzantine` (node ids), `committee_size`, `max_view`, `witnesses`.
  An unknown key is an error.
- Text after the header is for the reader and is ignored by the script.
- Every ```` ```diff ```` block is applied. `--- <file>` starts a change of that file; in the lines below, `-` is only in
  the old text, `+` only in the new text, a space in both (context to make the old text unique). The rest of the line
  is taken as is, indentation included. The old text must occur exactly once in the file, otherwise the mutant is
  reported as stale.

A mutant is checked either

- by simulation: `invariant`, `step` (and `model` for scope `all`) — `quint run` with these options, or
- by a test: `test` — the test from `scenarios_test.qnt`.

A simulation mutant may also list `witnesses` from `main.qnt`; the script reports in how many traces each one held
(e.g. `tail_fork_witness` — the block certified in view v is dropped by the certified block of view v + 1).

The exit code is 1 if any mutant is broken.

**Mutating Byzantine nodes.** A mutant with `byzantine` (and `committee_size`, `max_view`) calls `is_byzantine(id)` in
the new code. The script defines it in `types.qnt` and adds a model instance with `FAULTY = Set()` and
`BYZANTINE` = these nodes. `BYZANTINE` is a constant of `main.qnt` used only for this: `RUNNING = NODES \ FAULTY`
execute the protocol, the invariants are checked over `CORRECT = RUNNING \ BYZANTINE`; every regular model sets
`BYZANTINE = Set()`. `FAULTY` nodes are not executed; their messages come from `deliver_faulty`.

A mutation that rarely fires gives an almost vacuous `success`, so such a mutant counts its deviations in a ghost field
`c.deviations` of `Consensus`, incremented whenever a Byzantine node acts differently from a correct one (the script
refuses a mutant that does not). The witness `deviation_witness` reports in how many traces a Byzantine node deviated
at least once: `deviated in 1 of 300 traces`.

Mutants:

- [`equivocate`](tools/mutants/equivocate.md) — byzantine 3, safety_inv, expect safe
- [`equivocate_and_double_vote`](tools/mutants/equivocate_and_double_vote.md) — byzantine 3, safety_inv, expect safe
- [`lock_any`](tools/mutants/lock_any.md) — all, lockNeverDecreasesTest, expect violation
- [`no_direct_link_equivocate`](tools/mutants/no_direct_link_equivocate.md) — byzantine 3, safety_inv, expect safe
- [`no_direct_link_tail_fork`](tools/mutants/no_direct_link_tail_fork.md) — byzantine 3, safety_inv, expect safe
- [`no_lock_check`](tools/mutants/no_lock_check.md) — all, voteRespectsLockTest, expect violation
- [`no_lock_check_vs_old_qc`](tools/mutants/no_lock_check_vs_old_qc.md) — byzantine 3, lock_protection_inv, expect violation
- [`propose_old_qc`](tools/mutants/propose_old_qc.md) — byzantine 3, safety_inv, expect safe
- [`silent`](tools/mutants/silent.md) — byzantine 3, safety_inv, expect safe
- [`tail_fork`](tools/mutants/tail_fork.md) — byzantine 3, safety_inv, expect safe
- [`vote_past_lock`](tools/mutants/vote_past_lock.md) — byzantine 3, safety_inv, expect safe
- [`forging_server`](tools/mutants/forging_server.md) — byzantine 3, safety_inv, expect safe
- [`no_hash_check_vs_forging`](tools/mutants/no_hash_check_vs_forging.md) — byzantine 3, stored_genuine_inv, expect violation
- [`top_down_delivery`](tools/mutants/top_down_delivery.md) — all, n4f0v20t, stored_ancestry_inv, expect violation
- [`no_request_gate`](tools/mutants/no_request_gate.md) — all, syncGateTest, expect violation
- [`vote_on_synced`](tools/mutants/vote_on_synced.md) — all, blockSyncedStoresWithoutVoteTest, expect violation

`once_qc_in_view_inv` has no calibrating mutant: the vote collector keeps one vote per sender and at most one QC
per view, so a correct leader never builds a second QC in a view even if correct nodes vote twice
(`double_vote` on every node plus `equivocate` still holds). The voting rule itself has to be checked by a test.
