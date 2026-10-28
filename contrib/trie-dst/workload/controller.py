#!/usr/bin/env python3
"""Bedrock workload hooks; native state generation and checking live in Rust."""

import json
import os
import random
import shutil
import socket
import tempfile
from pathlib import Path
import signal
import subprocess
import sys
import time

# Persistence defaults match TreeConfig and EngineArgs in this Reth revision.
DEFAULTS = {"blocks": 64, "updates": 64, "proof_workers": 2, "timeout_secs": 60,
            "persistence_threshold": 50, "num_state_masking_blocks": 30,
            "memory_block_buffer_target": None, "crashes": 1, "cold_start": False}
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
    if type(result["cold_start"]) is not bool:
        raise ValueError("cold_start must be a boolean")
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
    else:
        for name in ("run.log", "controller.log", "done.json", "stdout.log", "init.log", "warm-ready.json", "data/blocks.jsonl"):
            source = STATE / name
            if source.exists():
                shutil.copyfile(source, destination.parent / ("reth-trie-dst-" + source.name))


def native(hook, request):
    binary = Path(__file__).with_name("reth-trie-dst")
    # Host input hooks need no coverage; the guest opts in through compose.yaml.
    env = os.environ.copy()
    env.pop("BEDROCK_COVERAGE_DIR", None)
    result = subprocess.run([str(binary), hook], input=json.dumps(request), text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, env=env)
    return json.loads(result.stdout)


def alive(pid):
    try:
        # kill(0) also succeeds for zombies; never report one as ready.
        return Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1][0] != "Z"
    except FileNotFoundError:
        return False


def barrier(message, expected, timeout=1, process=None):
    with tempfile.TemporaryDirectory(dir=STATE) as directory:
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as connection:
            connection.bind(str(Path(directory) / "reply"))
            deadline = time.monotonic() + timeout
            connection.sendto(message.encode(), str(STATE / "init.sock"))
            while True:
                connection.settimeout(max(0.001, min(0.1, deadline - time.monotonic())))
                try:
                    response = connection.recv(128)
                except TimeoutError:
                    if process is not None and (code := process.poll()) is not None:
                        raise RuntimeError(f"warm worker exited during handshake: exit={code}")
                    if time.monotonic() >= deadline:
                        raise
                    continue
                if response != expected:
                    raise RuntimeError("invalid initialization barrier response")
                return


class WarmProcess:
    """Adopt a guest process without retaining a host-side pipe or Popen object."""

    def __init__(self):
        self.pid = int((STATE / "warm.pid").read_text())

    def poll(self):
        status = STATE / "warm.exit"
        if status.exists():
            return int(status.read_text())
        if not alive(self.pid) and not alive(int((STATE / "launcher.pid").read_text())):
            return -signal.SIGKILL
        return None

    def send_signal(self, value):
        os.kill(self.pid, value)

    def kill(self):
        self.send_signal(signal.SIGKILL)

    def wait(self, timeout):
        deadline = time.monotonic() + timeout
        while self.poll() is None:
            if time.monotonic() >= deadline:
                raise TimeoutError("warm worker did not exit")
            time.sleep(0.01)
        return self.poll()


def launch():
    with (STATE / "stdout.log").open("wb") as output:
        process = subprocess.Popen(
            ["/usr/local/bin/thread-fuzz", "/usr/local/bin/reth-trie-dst",
             "@" + str(STATE / "init.json")], stdout=output, stderr=subprocess.STDOUT)
        tmp = STATE / "warm.pid.tmp"
        tmp.write_text(str(process.pid))
        tmp.replace(STATE / "warm.pid")
        code = process.wait()
        tmp = STATE / "warm.exit.tmp"
        tmp.write_text(str(code))
        tmp.replace(STATE / "warm.exit")


def warm_failure(message):
    details = [message]
    for name in ("launcher.pid", "warm.pid", "warm.exit", "launcher.log", "stdout.log"):
        path = STATE / name
        if path.exists():
            with path.open("rb") as source:
                source.seek(max(0, path.stat().st_size - 12000))
                details.append(f"{name}: {source.read(12000).decode(errors='replace')}")
    return "; ".join(details)


def warmup(request):
    args = arguments(request["args"])
    if args.pop("cold_start"):
        return {"ready": True, "detail": "cold_start: initialization remains in each execution"}
    args.pop("crashes")
    STATE.mkdir(parents=True, exist_ok=True)
    initial = args | {"datadir": str(STATE / "data"), "init_socket": str(STATE / "init.sock")}
    path = STATE / "init.json"
    if path.exists():
        if json.loads(path.read_text()) != initial:
            raise RuntimeError("warmup arguments changed")
    else:
        path.write_text(json.dumps(initial))
        (STATE / "warm.deadline").write_text(str(time.monotonic() + 120))
        with (STATE / "launcher.log").open("w") as output:
            process = subprocess.Popen([sys.executable, __file__, "launch"],
                                       stdout=output, stderr=subprocess.STDOUT,
                                       start_new_session=True)
        (STATE / "launcher.pid").write_text(str(process.pid))
    expired = time.monotonic() >= float((STATE / "warm.deadline").read_text())
    pid = STATE / "warm.pid"
    if not alive(int((STATE / "launcher.pid").read_text())):
        raise RuntimeError(warm_failure("warm launcher exited"))
    if not pid.exists():
        if expired:
            raise TimeoutError(warm_failure("warm initialization timed out"))
        return {"ready": False, "detail": "launching worker"}
    if not alive(int(pid.read_text())):
        raise RuntimeError(warm_failure("warm worker exited before start"))
    if not (STATE / "init.sock").exists():
        if expired:
            raise TimeoutError(warm_failure("warm initialization timed out"))
        return {"ready": False, "detail": "initializing runtime and empty storage"}
    try:
        barrier("ready", b"initialized")
    except (OSError, RuntimeError) as error:
        raise RuntimeError(warm_failure(f"warm readiness handshake failed: {error}")) from error
    # The acknowledged worker is idle, so its output offset is stable until start.
    prefix = (STATE / "stdout.log").read_bytes()
    (STATE / "init.log").write_bytes(prefix)
    (STATE / "warm-ready.json").write_text(json.dumps({
        "phase": "initialized/awaiting-input", "pid": int(pid.read_text()),
        "stdout_offset": len(prefix)}))
    return {"ready": True, "detail": "initialized/awaiting-input: live worker blocked on execution config"}


def read_output(source, process):
    data = source.read(65536)
    if data:
        return data
    if process.poll() is not None:
        # Exit may race the first EOF read; drain after observing termination.
        return source.read(65536)
    return None


def worker(config):
    process = None
    killed = 0
    recovered = 0
    recovered_persisted = 0
    plan = list(config["kill_blocks"])
    require_persisted = (bool(plan) and
                         config["workload"]["blocks"] >= config["workload"]["persistence_threshold"] + 4)
    persisted = False
    # Rust may make two sequential timeout-bounded waits before its next event;
    # recovery has no internal timeout and can use the campaign horizon.
    idle_budget = 2 * config["workload"]["timeout_secs"] + config["run_secs"]
    deadline = time.monotonic() + idle_budget
    waiting = False
    expected_blocks = config["workload"]["blocks"]
    try:
        workload = config["workload"] | {"datadir": str(STATE / "data"),
                                         "input_file": str(STATE / "pending-input.json")}
        workload_path = STATE / "workload.json"
        workload_path.write_text(json.dumps(workload))
        warm = (STATE / "warm.pid").exists()
        with (STATE / "run.log").open("wb") as output:
            while True:
                completed = False
                offset = 0
                if warm:
                    process = WarmProcess()
                    ready = json.loads((STATE / "warm-ready.json").read_text())
                    if ready["pid"] != process.pid:
                        raise RuntimeError("initialized worker PID changed")
                    offset = ready["stdout_offset"]
                    output.write((json.dumps({"warm": "reused", "pid": process.pid,
                                              "stdout_offset": offset}) + "\n").encode())
                    output.flush()
                    try:
                        barrier(str(workload_path), b"started",
                                timeout=max(0.001, deadline - time.monotonic()), process=process)
                    except (OSError, RuntimeError) as error:
                        raise RuntimeError(warm_failure(f"warm start handshake failed: {error}")) from error
                    warm = False
                else:
                    with (STATE / "stdout.log").open("wb") as stdout:
                        process = subprocess.Popen(
                            ["/usr/local/bin/thread-fuzz", "/usr/local/bin/reth-trie-dst",
                             "@" + str(workload_path)], stdout=stdout, stderr=subprocess.STDOUT)
                source = (STATE / "stdout.log").open("rb")
                source.seek(offset)
                pending = b""
                planned = False
                while True:
                    remaining = deadline - time.monotonic()
                    if waiting:
                        remaining = 1
                    elif remaining <= 0:
                        raise TimeoutError("workload recovery campaign timed out")
                    data = read_output(source, process)
                    if data == b"":
                        break
                    if data is None:
                        time.sleep(min(0.01, remaining))
                        continue
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
                        if event.get("input") == "restored":
                            expected_blocks = event["blocks"]
                        if event.get("input") == "injected":
                            waiting = False
                            # An injection replaces the remaining suffix, rather than appending it.
                            expected_blocks = event["block"] - 1 + event["blocks"]
                        if event.get("input") in ("injected", "restored"):
                            # A shorter suffix can make pending crash targets unreachable.
                            plan = [block for block in plan if block <= expected_blocks]
                            require_persisted = (bool(plan or require_persisted) and expected_blocks >=
                                                 config["workload"]["persistence_threshold"] + 4)
                        if event.get("completed"):
                            completed = True
                            assertion(event.get("blocks") == expected_blocks,
                                      json.dumps(event), "trie/input/completed")
                        if event.get("input") == "injected":
                            assertion(True, json.dumps(event), "trie/input/injected", "Sometimes")
                        if event.get("persistence") == "complete":
                            assertion(True, json.dumps(event), "trie/persistence/committed", "Sometimes")
                            persisted |= event.get("db_tip", 0) > 0
                        if event.get("recovery") == "complete":
                            recovered += 1
                            recovered_persisted += int(killed > 0 and event.get("head", 0) > 0)
                            assertion(True, json.dumps(event), "trie/recovery/checked", "Sometimes")
                        position = event.get("block", event.get("head", 0))
                        if (not planned and plan and position >= plan[0] and
                                ("operations" in event or
                                 event.get("persistence") == "complete") and
                                (persisted or not require_persisted)):
                            process.send_signal(signal.SIGKILL)
                            target = plan.pop(0)
                            killed += 1
                            planned = True
                            output.write((json.dumps({"nemesis": "kill", "target": target,
                                                      "block": position, "kills": killed}) + "\n").encode())
                            output.flush()
                code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
                source.close()
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
    if require_persisted:
        assertion(recovered_persisted > 0, f"post-crash recovery from persisted head: {recovered_persisted}",
                  "trie/recovery/persisted")
    (STATE / "done.json").write_text(json.dumps({"ok": ok, "kills": killed,
                                               "armed": killed + len(plan), "recoveries": recovered,
                                               "persisted_recoveries": recovered_persisted}))


def main():
    hook = sys.argv[1]
    value = sys.argv[2] if len(sys.argv) > 2 else None
    request = (json.load(sys.stdin) if value == "-" else
               json.loads(Path(value[1:]).read_text()) if value and value.startswith("@") else
               json.loads(value) if value else None)
    if hook == "mutation-operators":
        return native(hook, {})
    if hook in ("seed-inputs", "suffix-seed"):
        args = arguments(request["args"])
        args.pop("crashes")
        args.pop("cold_start")
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
            cold_start = args.pop("cold_start")
            supplied = request.get("base_input")
            if supplied is not None and len(supplied) > 32 << 20:
                raise ValueError("explicit input exceeds 32 MiB")
            input_value = json.loads(supplied) if supplied is not None else None
            if input_value is not None:
                validation = native("validate-input", {"input": input_value})
                args.update(blocks=validation["blocks"], expected_roots=validation["expected_roots"])
            kill_blocks = sorted(random.Random(request["seed"]).sample(
                range(1, args["blocks"] + 1), min(crashes, args["blocks"])))
            config = {"workload": args | {"seed": request["seed"], "input": input_value},
                      "kill_blocks": kill_blocks, "cold_start": cold_start,
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
                "finalized_by_start": decisions["config"]["run_secs"] == 1,
                "required": ["trie/input/completed"], "replays_exactly": True, "action": None,
                "notes": [], "host_decisions": []}
    if hook == "warmup":
        return warmup(request)
    if hook == "start":
        if not request.get("cold_start", False) and not (STATE / "warm.pid").exists():
            raise RuntimeError("warm worker was not initialized")
        STATE.mkdir(parents=True, exist_ok=True)
        if (STATE / "worker.pid").exists() or (STATE / "done.json").exists():
            raise RuntimeError("workload already started")
        if request["run_secs"] == 1:
            (STATE / "pending-input.finish").touch()
            worker(request)
            collect(Path("/bedrock/out/assertions.jsonl"))
        else:
            config_path = STATE / "config.json"
            config_path.write_text(json.dumps(request))
            with (STATE / "controller.log").open("w") as output:
                process = subprocess.Popen([sys.executable, __file__, "worker", "@" + str(config_path)],
                                           stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            (STATE / "worker.pid").write_text(str(process.pid))
    elif hook == "launch":
        launch()
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
            warm_pid = STATE / "warm.pid"
            if warm_pid.exists() and alive(int(warm_pid.read_text())):
                try:
                    os.kill(int(warm_pid.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            log = STATE / "run.log"
            assertion(False, "workload incomplete; " + (log.read_text()[-12000:] if log.exists() else "never started"))
        collect(Path("/bedrock/out/assertions.jsonl"))
    else:
        raise ValueError(f"unknown hook: {hook}")


if __name__ == "__main__":
    answer = main()
    if answer is not None:
        print(json.dumps(answer))
