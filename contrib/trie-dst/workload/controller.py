#!/usr/bin/env python3
"""Bedrock workload hooks; native state generation and checking live in Rust."""

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

DEFAULTS = {"blocks": 64, "updates": 64, "proof_workers": 2, "timeout_secs": 60}
LIMITS = {"blocks": (16, 1024), "updates": (1, 4096), "proof_workers": (1, 16),
          "timeout_secs": (1, 3600)}
STATE = Path("/tmp/reth-trie-dst")
ASSERTIONS = Path("/bedrock/assertions/reth-trie-dst.jsonl")


def arguments(values):
    unknown = values.keys() - DEFAULTS.keys()
    if unknown:
        raise ValueError(f"unknown arguments: {sorted(unknown)}")
    result = DEFAULTS | values
    for key, (low, high) in LIMITS.items():
        if type(result[key]) is not int or not low <= result[key] <= high:
            raise ValueError(f"{key} must be an integer in {low}..={high}")
    return result


def assertion(ok, detail):
    ASSERTIONS.parent.mkdir(parents=True, exist_ok=True)
    with ASSERTIONS.open("a") as output:
        record = {"condition": {"Bool": ok}, "result": ok,
                  "message": f"E1/reth-trie-dst: {detail}",
                  "location": {"file": "controller.py", "line": 35, "column": 1},
                  "timestamp_unix_nano": time.time_ns()}
        output.write(json.dumps({"Always": record}) + "\n")


def collect(destination, progress=False):
    offsets_path = Path("/bedrock/in/assertion-offsets.json")
    cursor = STATE / "progress-offsets.json"
    offsets = json.loads(offsets_path.read_text()) if offsets_path.exists() else {}
    if progress and cursor.exists():
        offsets = json.loads(cursor.read_text())
    records = []
    for path in sorted(Path("/bedrock/assertions").glob("*.jsonl")):
        with path.open("rb") as source:
            start = min(offsets.get(path.name, 0), path.stat().st_size)
            source.seek(start)
            data = source.read()
            complete = data.rfind(b"\n") + 1
            records.extend(data[:complete].decode().splitlines())
            offsets[path.name] = start + complete
    destination.parent.mkdir(parents=True, exist_ok=True)
    records.sort(key=lambda line: next(iter(json.loads(line).values())).get("timestamp_unix_nano", 0))
    destination.write_text("".join(line + "\n" for line in records))
    if progress:
        cursor.write_text(json.dumps(offsets))


def worker(config):
    try:
        with (STATE / "run.log").open("w") as output:
            result = subprocess.run(
                ["/usr/local/bin/thread-fuzz", "/usr/local/bin/reth-trie-dst",
                 json.dumps(config["workload"])], stdout=output, stderr=subprocess.STDOUT,
                timeout=max(1, config["run_secs"] - 5), check=False)
        ok = result.returncode == 0
        detail = f"exit={result.returncode}; " + (STATE / "run.log").read_text()[-12000:]
    except Exception as error:
        ok, detail = False, str(error)
    assertion(ok, detail)
    (STATE / "done.json").write_text(json.dumps({"ok": ok}))


def main():
    hook = sys.argv[1]
    request = json.load(sys.stdin) if len(sys.argv) > 2 and sys.argv[2] == "-" else (
        json.loads(sys.argv[2]) if len(sys.argv) > 2 else None)
    if hook == "parse-args":
        values = json.loads(Path(request["config_file"]).read_text()) if request.get("config_file") else {}
        for pair in request.get("args", []):
            key, value = pair.split("=", 1)
            values[key] = json.loads(value)
        return arguments(values)
    if hook == "config":
        if request.get("redraw") or request.get("drop"):
            raise ValueError("this workload has no mid-run semantic decisions to redraw or drop")
        decisions = request.get("decisions")
        if decisions is None:
            config = {"workload": arguments(request["args"]) | {"seed": request["seed"]},
                      "run_secs": request["campaign"]["run_secs"]}
            decisions = {"config": config, "seed": request["seed"],
                         "rng_seed": request["seed"], "preempt": request["preempt"]}
        decisions = dict(decisions)
        if request.get("rng_seed") is not None:
            decisions["rng_seed"] = request["rng_seed"]
        return {"config": decisions["config"], "decisions": decisions,
                "run": {"seed": decisions["seed"], "rng_seed": decisions["rng_seed"],
                        "run_secs": decisions["config"]["run_secs"],
                        "preempt": decisions["preempt"], "swarm": {}},
                "required": [], "replays_exactly": True, "action": None,
                "notes": [], "host_decisions": []}
    if hook == "warmup":
        return {"ready": True, "detail": "reth-trie-dst controller ready; each branch starts with empty state"}
    if hook == "start":
        STATE.mkdir(parents=True, exist_ok=True)
        if (STATE / "worker.pid").exists():
            raise RuntimeError("workload already started")
        with (STATE / "controller.log").open("w") as output:
            process = subprocess.Popen([sys.executable, __file__, "worker", json.dumps(request)],
                                       stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        (STATE / "worker.pid").write_text(str(process.pid))
    elif hook == "worker":
        worker(request)
    elif hook == "progress":
        collect(Path("/bedrock/out/progress.jsonl"), progress=True)
    elif hook == "finalize":
        if not (STATE / "done.json").exists():
            pid = STATE / "worker.pid"
            if pid.exists():
                try:
                    os.killpg(int(pid.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            log = STATE / "run.log"
            assertion(False, "workload incomplete; " + (log.read_text()[-12000:] if log.exists() else "never started"))
        out = Path("/bedrock/out")
        out.mkdir(parents=True, exist_ok=True)
        for name in ["run.log", "controller.log", "done.json"]:
            if (STATE / name).exists():
                shutil.copyfile(STATE / name, out / ("reth-trie-dst-" + name))
        collect(out / "assertions.jsonl")
    elif hook == "observe":
        return {"decisions": request["decisions"], "decision_diff": (["recorded decisions differ"] if request.get("original") is not None
                                  and request["original"] != request["decisions"] else []), "moments": [],
                "events_origin_ns": None}
    else:
        raise ValueError(f"unknown hook: {hook}")


if __name__ == "__main__":
    answer = main()
    if answer is not None:
        print(json.dumps(answer))
