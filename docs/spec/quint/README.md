### Make targets

The commands below are wrapped in `Makefile`; the GitHub Actions workflows (`spec-tests`, `spec-mutants`,
`spec-sim`, run manually) call the same targets.

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

- `--main` — configuration from `models.qnt` (`n4f1v6`, `n4f1v20`, `n7f2v6`).
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
```

Options:

- `no_lock_check lock_any` — run only the listed mutants (default: all).
- `--samples` / `--max-steps` — number and length of traces for simulation (default 300 / 300).
- `--seed=0x...` — reproduce a run.
- `--verbose` — print the Quint output.
- `--entries` — another TOML file with mutants.

Mutants are listed in `tools/mutants.toml`. `changes` is a list of replacements of the string `old` with `new`
in `file` (each string must occur exactly once, otherwise the mutant is reported as stale). A mutant is checked either

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

| mutant | scope | change in the protocol | checked by | expect | status |
|---|---|---|---|---|---|
| `no_lock_check` | all | `consensus.qnt`: `c.voted_view < view and block.justify.view >= c.qc_lock.view` → `c.voted_view < view` | test `voteRespectsLockTest` | violation | success (simulation with `lock_protection_inv` does not catch it: the faulty node rarely proposes on an old QC and a fork QC needs a quorum) |
| `lock_any` | all | `consensus.qnt`: `if (b_star.justify.view > c.qc_lock.view)` → `if (true)` | test `lockNeverDecreasesTest` | violation | success (also caught by simulation with `lock_monotonic_inv`) |
| `vote_past_lock` | `n3` of n = 4 | `consensus.qnt`: `c.voted_view < view and block.justify.view >= c.qc_lock.view` → `c.voted_view < view and (block.justify.view >= c.qc_lock.view or is_byzantine(c.id))` | simulation, `safety_inv`, `step` | safe | success, but almost vacuously: correct leaders rarely propose below a lock, the Byzantine node deviated in 1–6 of 300 traces with `step` and never with `step_fair` |
| `propose_old_qc` | `n3` of n = 4 | `consensus.qnt`, `on_propose`: a Byzantine leader builds its block on the genesis QC instead of `high_qc` (`val qc = if (is_byzantine(c.id)) genesis_qc else high_qc`) | simulation, `safety_inv`, `step` | safe | success; deviates in almost every trace (299 of 300), also with `step_fair` and together with `vote_past_lock` |
| `no_lock_check_vs_old_qc` | `no_lock_check` on all nodes, `propose_old_qc` on `n3` of n = 4 | both changes above | simulation, `lock_protection_inv`, `step` | violation | success: with a leader that proposes on the genesis QC, correct replicas without the lock check certify a fork of a committed block (also with `step_fair`) |
| `silent` | `n3` of n = 4 | `sm.qnt`, `apply_state`: `net: res.net.foldl(received.net, ...)` → `net: if (is_byzantine(state.node_id)) received.net else res.net.foldl(received.net, ...)` — the node handles everything but sends nothing (no votes, proposals, wishes, statuses, TC relays) | simulation, `safety_inv`, `step` | safe | success, deviates in every trace, also with `step_fair`; commits still happen with `step_fair` |
| `tail_fork` | `n3` of n = 4 | `consensus.qnt`, `on_propose`: a Byzantine leader of v + 1 builds on its lock QC(v − 1) instead of `high_qc` = QC(v) (`val qc = if (is_byzantine(c.id)) c.qc_lock else high_qc`), dropping the block of view v | simulation, `safety_inv`, `step`, witness `tail_fork_witness` | safe | success; deviates in 295 of 300 traces, the fork gets a QC (`tail_fork_witness`) in 276 of 300 (24 of 300 with all nodes correct) |
| `equivocate` | `n3` of n = 4 | `driver.qnt`, `on_propose_request`: a Byzantine leader sends every node two blocks of its view (hash versions 0 and 1) instead of one broadcast; even nodes get version 0 first, odd nodes version 1 first | simulation, `safety_inv`, `step` | safe | success; deviates in almost every trace |
| `equivocate_and_double_vote` | `n3` of n = 4 | `equivocate` plus `consensus.qnt`, `can_vote`: `c.voted_view < view` → `(c.voted_view < view or is_byzantine(c.id))` — the Byzantine node votes for both blocks | simulation, `safety_inv`, `step` | safe | success |

`once_qc_in_view_inv` has no calibrating mutant: the vote collector keeps one vote per sender and at most one QC
per view, so a correct leader never builds a second QC in a view even if correct nodes vote twice
(`double_vote` on every node plus `equivocate` still holds). The voting rule itself has to be checked by a test.
