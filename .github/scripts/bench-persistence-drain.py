#!/usr/bin/env python3
"""Bounded post-measurement settling, followed by a shutdown trace coverage gate.

Settling does not force persistence of the retained in-memory buffer. The node's
normal SIGTERM handler drains that buffer; audit rejects an incomplete drain.
"""
import argparse
import json
from pathlib import Path
import re
import time
from urllib.request import Request, urlopen


def latest_block(url):
    request = Request(url, data=json.dumps({"jsonrpc": "2.0", "id": 1,
                      "method": "eth_getBlockByNumber", "params": ["latest", False]}).encode(),
                      headers={"Content-Type": "application/json"})
    with urlopen(request, timeout=5) as response:
        reply = json.load(response)
    if "error" in reply or not reply.get("result"):
        raise ValueError(f"Cannot read drain target: {reply}")
    block = reply["result"]
    return {"number": int(block["number"], 16), "hash": block["hash"]}


def settle(args):
    output = Path(args.output_dir) / "persistence-drain.json"
    started = time.monotonic()
    data = {"started_unix_ms": time.time_ns() // 1_000_000,
            "settle_seconds": args.seconds, "target": latest_block(args.rpc_url),
            "observations": [], "coverage": "pending post-shutdown trace audit",
            "note": "settle permits backlog progress; retained buffer still drains at SIGTERM"}
    try:
        while True:
            observation = {"elapsed_seconds": time.monotonic() - started}
            if args.metrics_url:
                try:
                    with urlopen(args.metrics_url, timeout=2) as response:
                        lines = response.read().decode().splitlines()
                    observation["persistence_metrics"] = [line for line in lines if
                        not line.startswith("#") and
                        ("persistence_save_blocks" in line or "database_save_blocks_mdbx" in line)]
                except OSError as exc:
                    observation["metrics_error"] = str(exc)
            data["observations"].append(observation)
            remaining = args.seconds - (time.monotonic() - started)
            if remaining <= 0:
                break
            time.sleep(min(1, remaining))
        data["head_after_settle"] = latest_block(args.rpc_url)
        if data["head_after_settle"] != data["target"]:
            raise ValueError("Canonical head changed during post-replay settling")
    finally:
        data["settle_elapsed_seconds"] = time.monotonic() - started
        data["ended_unix_ms"] = time.time_ns() // 1_000_000
        output.write_text(json.dumps(data, indent=2) + "\n")


def audit(args):
    root = Path(args.output_dir)
    output = root / "persistence-drain.json"
    data = json.loads(output.read_text())
    try:
        report = json.loads((root / "report.json").read_text())
        warmup = root / "txgen" / "warmup-report.json"
        expected = len(report["blocks"])
        if warmup.exists():
            expected += len(json.loads(warmup.read_text())["blocks"])
        target = data["target"]
        if report["blocks"][-1]["number"] != target["number"]:
            raise ValueError("Measured report's final block does not match drain target")
        trace = json.loads((root / "tracing-chrome-profile.json").read_text())
        events = trace if isinstance(trace, list) else trace["traceEvents"]
        tids = {(e["pid"], e["tid"]) for e in events if e.get("name") == "thread_name"
                and e.get("args", {}).get("name") == "persistence"}
        stacks = {tid: [] for tid in tids}
        completed = []
        for event in events:
            key = (event.get("pid"), event.get("tid"))
            if key not in tids:
                continue
            stack = stacks[key]
            if event["ph"] == "B":
                stack.append({"event": event, "endpoint": None})
            elif event["ph"] == "i" and event.get("args", {}).get("message") == "Saved range of blocks":
                batch = next((x for x in reversed(stack) if x["event"]["name"] == "on_save_blocks"), None)
                if batch:
                    endpoint = re.search(r"number: (\d+), hash: (0x[0-9a-fA-F]+)", event["args"]["last"])
                    if endpoint:
                        batch["endpoint"] = {"number": int(endpoint[1]), "hash": endpoint[2]}
            elif event["ph"] == "E":
                batch = stack.pop()
                if batch["event"]["name"] != event["name"]:
                    raise ValueError("Unbalanced persistence trace spans")
                if event["name"] == "on_save_blocks":
                    completed.append({"blocks": int(batch["event"]["args"]["block_count"]),
                                      "seconds": (event["ts"] - batch["event"]["ts"]) / 1e6,
                                      "endpoint": batch["endpoint"]})
        unclosed = [x["event"]["name"] for stack in stacks.values() for x in stack]
        observed = sum(batch["blocks"] for batch in completed)
        data["audit"] = {"expected_blocks": expected, "completed_blocks": observed,
                         "completed_batches": completed, "unclosed_spans": unclosed}
        if observed != expected or not completed or completed[-1]["endpoint"] != target or "on_save_blocks" in unclosed:
            raise ValueError(f"Persistence incomplete: {observed}/{expected} blocks; target {target}")
        data["coverage"] = "complete replay, including retained-buffer shutdown persistence"
    except (OSError, ValueError, KeyError, IndexError) as exc:
        data["coverage"] = "failed"
        data["audit_error"] = str(exc)
        raise
    finally:
        output.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["settle", "audit"])
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--rpc-url", default="http://127.0.0.1:8545")
    parser.add_argument("--metrics-url")
    parser.add_argument("--seconds", type=float, default=10)
    args = parser.parse_args()
    if not 0 < args.seconds <= 60:
        parser.error("--seconds must be between 0 and 60")
    (settle if args.mode == "settle" else audit)(args)
