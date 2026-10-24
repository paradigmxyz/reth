#!/usr/bin/env python3
"""Bedrock workload hooks; native state generation and checking live in Rust."""

import json
import os
import random
import select
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

# Persistence defaults match TreeConfig and EngineArgs in this Reth revision.
DEFAULTS = {"blocks": 64, "updates": 64, "proof_workers": 2, "timeout_secs": 60,
            "persistence_threshold": 50, "num_state_masking_blocks": 30,
            "memory_block_buffer_target": None, "crashes": 1}
LIMITS = {"crashes": (0, 16), "blocks": (1, 1024), "updates": (1, 4096), "proof_workers": (1, 16),
          "timeout_secs": (1, 3600), "persistence_threshold": (0, 2**64 - 2),
          "num_state_masking_blocks": (0, 2**64 - 1),
          "memory_block_buffer_target": (0, 2**64 - 1)}
STATE = Path("/tmp/reth-trie-dst")
ASSERTIONS = Path("/bedrock/assertions/reth-trie-dst.jsonl")


def arguments(values):
    unknown = values.keys() - DEFAULTS.keys()
    if unknown:
        raise ValueError(f"unknown arguments: {sorted(unknown)}")
    result = DEFAULTS | values
    for key, (low, high) in LIMITS.items():
        if key == "memory_block_buffer_target" and result[key] is None:
            continue
        if type(result[key]) is not int or not low <= result[key] <= high:
            raise ValueError(f"{key} must be an integer in {low}..={high}")
    if result["crashes"] > result["blocks"]:
        raise ValueError("crashes must not exceed blocks")
    threshold = result["persistence_threshold"]
    buffer = result["memory_block_buffer_target"]
    if buffer is None:
        buffer = min(5, threshold)
    masking = result["num_state_masking_blocks"] if threshold else 0
    if buffer > threshold:
        raise ValueError("buffer must not exceed persistence threshold")
    if masking and masking + buffer >= threshold:
        raise ValueError("masking blocks + buffer must be less than persistence threshold")
    return result


def assertion(ok, detail, signature="trie/run/checked", kind="Always"):
    ASSERTIONS.parent.mkdir(parents=True, exist_ok=True)
    with ASSERTIONS.open("a") as output:
        record = {"condition": {"Bool": ok}, "result": ok,
                  "message": f"{signature}: {detail}",
                  "location": {"file": "controller.py", "line": 35, "column": 1},
                  "timestamp_unix_nano": time.time_ns()}
        output.write(json.dumps({kind: record}) + "\n")


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


def native(hook, request):
    binary = Path(__file__).with_name("reth-trie-dst")
    # Host input hooks need no coverage; the guest opts in through compose.yaml.
    env = os.environ.copy()
    env.pop("BEDROCK_COVERAGE_DIR", None)
    result = subprocess.run([str(binary), hook], input=json.dumps(request), text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, env=env)
    return json.loads(result.stdout)


def worker(config):
    process = None
    killed = 0
    recovered = 0
    plan = list(config["kill_blocks"])
    # Rust may make two sequential timeout-bounded waits before its next event;
    # recovery has no internal timeout and can use the campaign horizon.
    idle_budget = 2 * config["workload"]["timeout_secs"] + config["run_secs"]
    deadline = time.monotonic() + idle_budget
    waiting = False
    diagnostic = None
    try:
        workload = config["workload"] | {"datadir": str(STATE / "data"),
                                         "input_file": str(STATE / "pending-input.json")}
        (STATE / "config.json").write_text(json.dumps(config))
        with (STATE / "run.log").open("wb") as output:
            while True:
                completed = False
                process = subprocess.Popen(
                    ["/usr/local/bin/thread-fuzz", "/usr/local/bin/reth-trie-dst",
                     "-"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                process.stdin.write(json.dumps(workload).encode())
                process.stdin.close()
                pending = b""
                planned = False
                while True:
                    remaining = deadline - time.monotonic()
                    if waiting:
                        remaining = 1
                    elif remaining <= 0:
                        raise TimeoutError("workload recovery campaign timed out")
                    if not select.select([process.stdout], [], [], remaining)[0]:
                        continue
                    data = os.read(process.stdout.fileno(), 65536)
                    if not data:
                        break
                    output.write(data)
                    pending += data
                    while b"\n" in pending:
                        line, pending = pending.split(b"\n", 1)
                        try:
                            event = json.loads(line)
                        except (ValueError, UnicodeDecodeError):
                            continue  # Rust errors/backtraces share stdout with JSON events.
                        if not isinstance(event, dict):
                            continue
                        deadline = time.monotonic() + idle_budget
                        if event.get("input") == "waiting":
                            waiting = True
                            assertion(True, json.dumps(event), "trie/input/checked", "Sometimes")
                        if event.get("input") == "injected":
                            waiting = False
                        if event.get("completed"):
                            completed = True
                            diagnostic = event.get("diagnostic_ms")
                            assertion(event.get("blocks") == config["workload"]["blocks"],
                                      json.dumps(event), "trie/input/completed")
                        if event.get("input") == "injected":
                            assertion(True, json.dumps(event), "trie/input/injected", "Sometimes")
                        if event.get("persistence") == "complete":
                            assertion(True, json.dumps(event), "trie/persistence/committed", "Sometimes")
                        if event.get("recovery") == "complete":
                            recovered += 1
                            assertion(True, json.dumps(event), "trie/recovery/checked", "Sometimes")
                        if not planned and plan and "operations" in event and event.get("block", 0) >= plan[0]:
                            process.send_signal(signal.SIGKILL)
                            target = plan.pop(0)
                            killed += 1
                            planned = True
                            output.write((json.dumps({"nemesis": "kill", "target": target,
                                                      "block": event["block"], "kills": killed}) + "\n").encode())
                            output.flush()
                code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
                process.stdout.close()
                if planned:
                    if code != -signal.SIGKILL:
                        raise RuntimeError(f"planned SIGKILL did not kill worker: exit={code}")
                    continue
                if code != 0:
                    raise RuntimeError(f"unexpected worker exit={code}")
                if not completed:
                    raise RuntimeError("worker exited without completing its input")
                if plan:
                    raise RuntimeError(f"nemesis did not fire at blocks {plan}")
                break
        ok = True
        detail = f"completed after {killed} SIGKILL/restarts"
    except Exception as error:
        ok, detail = False, str(error)
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=5)
    if not ok:
        detail += "; " + (STATE / "run.log").read_text(errors="replace")[-12000:]
    assertion(ok, detail)
    (STATE / "done.json").write_text(json.dumps({"ok": ok, "kills": killed,
                                               "armed": len(config["kill_blocks"]), "recoveries": recovered}))
    return {"diagnostic_ms": diagnostic}


def main():
    hook = sys.argv[1]
    value = sys.argv[2] if len(sys.argv) > 2 else None
    request = (json.load(sys.stdin) if value == "-" else
               json.loads(Path(value[1:]).read_text()) if value and value.startswith("@") else
               json.loads(value) if value else None)
    if hook in ("mutation-operators", "mutation-sites"):
        if hook == "mutation-operators":
            return native(hook, {})
        return native(hook, {"input": json.loads(request["input"]),
                             "operator": request["operator"]})
    if hook in ("seed-inputs", "suffix-seed", "mutate-input"):
        args = arguments(request["args"])
        args.pop("crashes")
        if hook == "mutate-input":
            return json.dumps(native("mutate-input", {
                "input": json.loads(request["input"]), "seed": request["seed"],
                "operator": request.get("operator") or None, "path": request.get("path", []),
                "donor": json.loads(request["donor"]) if request.get("donor") else None,
                "donor_path": request.get("donor_path", []),
            }))
        if hook == "suffix-seed":
            # Each block establishes its own accounts, so it also runs at a deep snapshot.
            args.update(blocks=4, updates=min(4080, args["updates"]))
        inputs = [json.dumps(native("generate-suffix" if hook == "suffix-seed" else "generate-input", args | {"seed": seed}))
                  for seed in range(1 if hook == "suffix-seed" else 8)]
        return inputs[0] if hook == "suffix-seed" else inputs
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
        if decisions is not None and request.get("base_input") is not None:
            raise ValueError("base_input cannot replace recorded decisions")
        if decisions is None:
            args = arguments(request["args"])
            crashes = args.pop("crashes")
            supplied = request.get("base_input")
            if supplied is not None and len(supplied) > 32 << 20:
                raise ValueError("explicit input exceeds 32 MiB")
            input_value = native("validate-input", {"input": json.loads(supplied)}) if supplied is not None else None
            if input_value is not None:
                args["blocks"] = sum(op["kind"] == "commit" for op in input_value["operations"])
            kill_blocks = sorted(random.Random(request["seed"]).sample(
                range(1, args["blocks"] + 1), min(crashes, args["blocks"])))
            config = {"workload": args | {"seed": request["seed"], "input": input_value},
                      "kill_blocks": kill_blocks, "run_secs": request["campaign"]["run_secs"]}
            decisions = {"config": config, "seed": request["seed"],
                         "rng_seed": request["seed"], "preempt": request["preempt"]}
        decisions = dict(decisions)
        if request.get("rng_seed") is not None:
            decisions["rng_seed"] = request["rng_seed"]
        return {"config": decisions["config"], "decisions": decisions,
                "run": {"seed": decisions["seed"], "rng_seed": decisions["rng_seed"],
                        "run_secs": decisions["config"]["run_secs"],
                        "preempt": decisions["preempt"], "swarm": {}},
                "required": ["trie/input/completed"], "replays_exactly": True, "action": None,
                "notes": [], "host_decisions": []}
    if hook == "warmup":
        return {"ready": True, "detail": "reth-trie-dst controller ready; each branch starts with empty state"}
    if hook == "start":
        STATE.mkdir(parents=True, exist_ok=True)
        if (STATE / "worker.pid").exists():
            raise RuntimeError("workload already started")
        if request["run_secs"] == 1:
            (STATE / "pending-input.finish").touch()
            return worker(request)
        else:
            config_path = STATE / "config.json"
            config_path.write_text(json.dumps(request))
            with (STATE / "controller.log").open("w") as output:
                process = subprocess.Popen([sys.executable, __file__, "worker", "@" + str(config_path)],
                                           stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            (STATE / "worker.pid").write_text(str(process.pid))
    elif hook == "worker":
        worker(request)
    elif hook == "set-input":
        if (STATE / "done.json").exists():
            raise RuntimeError("input injection reached an already completed workload")
        pending = STATE / "pending-input.json"
        tmp = pending.with_suffix(".tmp")
        applied = STATE / "data/input-overrides.json"
        previous = [json.loads(path.read_text())[key] for path, key in
                    ((pending, "id"), (applied, "applied")) if path.exists()]
        tmp.write_text(json.dumps({"id": max(previous, default=0) + 1, "input": request}))
        tmp.replace(pending)
    elif hook == "merge-assertions":
        collect(Path("/bedrock/out/assertions.jsonl"))
    elif hook == "progress":
        collect(Path("/bedrock/out/progress.jsonl"), progress=True)
    elif hook == "finalize":
        if STATE.exists():
            (STATE / "pending-input.finish").touch()
        deadline = time.monotonic() + 5
        while STATE.exists() and not (STATE / "done.json").exists() and time.monotonic() < deadline:
            time.sleep(0.01)
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
        for name in ["run.log", "controller.log", "done.json", "data/blocks.jsonl", "data/input-overrides.json", "pending-input.json", "config.json"]:
            if (STATE / name).exists():
                shutil.copyfile(STATE / name, out / ("reth-trie-dst-" + Path(name).name))
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
