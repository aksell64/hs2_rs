### Run model

    quint run models.qnt --main=n4f1v20 --step=step_gst --invariant=safety_inv \
    --max-steps=1000 --max-samples=1 --out-itf=trace.itf.json

This runs the Quint simulator on a random trace and checks safety after GST.
The trace is saved to `trace.itf.json` in the Informal Trace Format.

It’s better to set `--max-steps` > 500 so that committed blocks appear in sufficient numbers.

Options:

- `--main` — configuration from `models.qnt` (`n4f1v6`, `n4f1v20`, `n7f2v6`).
- `--step` — `step_gst` (timers fire only after the leader of the view has proposed, for commits and progress)
  or `step` (timers fire at any time, for safety without the synchrony assumption).
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
- Use `--step=step` for safety without the synchrony assumption, `--step=step_gst` to check safety on traces with many commits.
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
