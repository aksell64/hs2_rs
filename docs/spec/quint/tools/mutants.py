#!/usr/bin/env python3
import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SPEC_DIR = Path(__file__).resolve().parent.parent

ENTRIES_DIR = Path(__file__).resolve().parent / "mutants"
INT_KEYS = {"committee_size", "max_view"}
LIST_KEYS = {"witnesses"}
KEYS = {"expect", "test", "byzantine", "model", "invariant", "step"} | INT_KEYS | LIST_KEYS


def parse_diff(name: str, block: list[str]) -> list[dict]:
    changes = []
    for line in block:
        if line.startswith("--- "):
            changes.append({"file": line[4:].strip(), "lines": []})
        elif not changes:
            if line.strip():
                raise RuntimeError(f"{name}: diff line before `--- file`: {line!r}")
        elif not line:
            changes[-1]["lines"].append((" ", ""))
        elif line[0] in "-+ ":
            changes[-1]["lines"].append((line[0], line[1:]))
        else:
            raise RuntimeError(f"{name}: diff line must start with '-', '+' or ' ': {line!r}")
    result = []
    for change in changes:
        lines = change["lines"]
        while lines and lines[-1] == (" ", ""):
            lines.pop()
        old = [text for prefix, text in lines if prefix != "+"]
        new = [text for prefix, text in lines if prefix != "-"]
        result.append({"file": change["file"], "old": "\n".join(old), "new": "\n".join(new)})
    return result


def parse_entry(path: Path) -> dict:
    lines = path.read_text().split("\n")
    if not lines[0].startswith("# "):
        raise RuntimeError(f"{path.name}: the first line must be `# <name>`")
    entry = {"name": lines[0][2:].strip()}
    if entry["name"] != path.stem:
        raise RuntimeError(f"{path.name}: name {entry['name']!r} differs from the file name")
    i = 1
    while i < len(lines) and not lines[i].strip():
        i += 1
    while i < len(lines) and lines[i].strip():
        key, sep, value = lines[i].partition(": ")
        if not sep or key not in KEYS:
            raise RuntimeError(f"{path.name}: unknown header line {lines[i]!r}")
        value = value.strip()
        if key == "byzantine":
            entry[key] = [int(v) for v in value.split(",")]
        elif key in INT_KEYS:
            entry[key] = int(value)
        elif key in LIST_KEYS:
            entry[key] = [v.strip() for v in value.split(",")]
        else:
            entry[key] = value
        i += 1
    entry["changes"] = []
    block = None
    for line in lines[i:]:
        if block is None:
            if line.strip() == "```diff":
                block = []
        elif line.strip() == "```":
            entry["changes"] += parse_diff(entry["name"], block)
            block = None
        else:
            block.append(line)
    if block is not None:
        raise RuntimeError(f"{path.name}: unclosed diff block")
    if not entry["changes"]:
        raise RuntimeError(f"{path.name}: no diff block")
    return entry


def load_entries(directory: Path) -> list[dict]:
    return [parse_entry(path) for path in sorted(directory.glob("*.md"))]
BYZANTINE_MODEL = "byzantine_model"


def apply_changes(spec: Path, mutant: dict) -> None:
    for change in mutant["changes"]:
        path = spec / change["file"]
        text = path.read_text()
        if text.count(change["old"]) != 1:
            raise RuntimeError(f"{mutant['name']}: pattern not found exactly once in {change['file']}, entry is stale")
        path.write_text(text.replace(change["old"], change["new"]))


DEVIATION_WITNESS = "deviation_witness"


def insert_once(path: Path, anchor: str, text: str) -> None:
    content = path.read_text()
    if content.count(anchor) != 1:
        raise RuntimeError(f"anchor not found exactly once in {path.name}: {anchor!r}")
    path.write_text(content.replace(anchor, anchor + text))


def add_byzantine_setup(spec: Path, mutant: dict) -> None:
    if not any("deviations" in change["new"] for change in mutant["changes"]):
        raise RuntimeError(f"{mutant['name']}: a mutation of Byzantine nodes must count its deviations in c.deviations")
    nodes = ", ".join(str(n) for n in mutant["byzantine"])
    types = spec / "types.qnt"
    text = types.read_text().rstrip()
    assert text.endswith("}")
    types.write_text(text[:-1] + f"  pure def is_byzantine(id: NodeID): bool = Set({nodes}).contains(id)\n}}\n")
    consensus = spec / "consensus.qnt"
    insert_once(consensus, "    id: NodeID,\n", "    deviations: int,\n")
    insert_once(consensus, "      id: id,\n", "      deviations: 0,\n")
    main = spec / "main.qnt"
    text = main.read_text().rstrip()
    assert text.endswith("}")
    main.write_text(text[:-1] + f"""
  val {DEVIATION_WITNESS} = BYZANTINE.exists(id => nodes.get(id).driver.consensus.deviations > 0)
}}
""")
    models = spec / "models.qnt"
    models.write_text(models.read_text() + f"""
module {BYZANTINE_MODEL} {{
  import main(committee_size = {mutant['committee_size']}, FAULTY = Set(), BYZANTINE = Set({nodes}), MAX_VIEW = {mutant['max_view']}, FAULTY_PERCENT = 0, TIMER_PERCENT = 20).* from "./main"
}}
""")


def run_mutant(mutant: dict, samples: int, max_steps: int, seed: str | None) -> dict:
    with tempfile.TemporaryDirectory() as tmp:
        spec = Path(tmp) / "quint"
        shutil.copytree(SPEC_DIR, spec, ignore=shutil.ignore_patterns("tools", "*.itf.json", "*.md"))
        apply_changes(spec, mutant)
        if "byzantine" in mutant:
            add_byzantine_setup(spec, mutant)
        if "test" in mutant:
            cmd = [
                "quint", "test", "scenarios_test.qnt",
                "--main=scenariosTest",
                f"--match=^{mutant['test']}$",
            ]
        else:
            cmd = [
                "quint", "run", "models.qnt",
                f"--main={BYZANTINE_MODEL if 'byzantine' in mutant else mutant['model']}",
                f"--step={mutant['step']}",
                f"--invariant={mutant['invariant']}",
                f"--max-steps={max_steps}",
                f"--max-samples={samples}",
                "--verbosity=1",
            ]
            witnesses = ([DEVIATION_WITNESS] if "byzantine" in mutant else []) + mutant.get("witnesses", [])
            if witnesses:
                cmd += ["--witnesses", *witnesses]
        if seed:
            cmd.append(f"--seed={seed}")
        proc = subprocess.run(cmd, cwd=spec, capture_output=True, text=True)
        output = proc.stdout + proc.stderr
    if "test" in mutant:
        if "passing" not in output and "failed after" not in output:
            raise RuntimeError(f"{mutant['name']}: quint test did not run\n{output}")
        violated = "failed after" in output
    else:
        if "[violation]" not in output and "[ok]" not in output:
            raise RuntimeError(f"{mutant['name']}: quint run did not run\n{output}")
        violated = "[violation]" in output
    if mutant["expect"] not in ("violation", "safe"):
        raise RuntimeError(f"{mutant['name']}: expect must be \"violation\" or \"safe\"")
    bad = violated != (mutant["expect"] == "violation")
    status = "broken" if bad else "success"
    found_seed = re.search(r"--seed=(0x[0-9a-f]+)", output)
    deviations = None
    if "byzantine" in mutant:
        found = re.search(rf"{DEVIATION_WITNESS} was witnessed in (\d+) trace", output)
        deviations = found.group(1) if found else "?"
    witnessed = {}
    for witness in mutant.get("witnesses", []):
        found = re.search(rf"{witness} was witnessed in (\d+) trace", output)
        witnessed[witness] = found.group(1) if found else "?"
    return {
        "deviations": deviations,
        "witnessed": witnessed,
        "name": mutant["name"],
        "check": mutant.get("test", mutant.get("invariant")),
        "status": status,
        "bad": bad,
        "seed": found_seed.group(1) if found_seed else "-",
        "output": output,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run mutants of the Quint spec and check the expected outcome.")
    parser.add_argument("names", nargs="*", help="entries to run (default: all)")
    parser.add_argument("--samples", type=int, default=300)
    parser.add_argument("--max-steps", type=int, default=300)
    parser.add_argument("--seed")
    parser.add_argument("--verbose", action="store_true", help="print quint output")
    parser.add_argument("--entries", type=Path, default=ENTRIES_DIR, help="directory with mutant files")
    args = parser.parse_args()

    entries = load_entries(args.entries)
    unknown = set(args.names) - {m["name"] for m in entries}
    if unknown:
        parser.error(f"unknown entries: {', '.join(sorted(unknown))}")
    selected = [m for m in entries if not args.names or m["name"] in args.names]
    bad = 0
    for mutant in selected:
        result = run_mutant(mutant, args.samples, args.max_steps, args.seed)
        deviations = "" if result["deviations"] is None else f"deviated in {result['deviations']} of {args.samples} traces "
        scope = f"byz={mutant['byzantine']}" if "byzantine" in mutant else "all"
        print(f"{result['name']:<20} {scope:<9} {result['check']:<24} {result['status']:<9} "
              f"{deviations}seed={result['seed']}")
        for witness, count in result["witnessed"].items():
            print(f"{'':<20} {witness} in {count} of {args.samples} traces")
        if args.verbose:
            print(result["output"])
        bad += result["bad"]
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
