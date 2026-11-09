#!/usr/bin/env python3
"""Smoke-test stages against a local peer using the RPC compatibility fixture."""

import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time


RETH = sys.argv[1] if len(sys.argv) > 1 else "reth"
FIXTURE = (
    Path(__file__).resolve().parents[2]
    / "crates/rpc/rpc-e2e-tests/testdata/rpc-compat"
)
TIP = 45
STAGES = {
    "headers": "Headers",
    "bodies": "Bodies",
    "senders": "SenderRecovery",
    "execution": "Execution",
    "merkle": "MerkleExecute",
    "tx-lookup": "TransactionLookup",
    "account-history": "IndexAccountHistory",
    "storage-history": "IndexStorageHistory",
}


def run(*args, quiet=False):
    command = [RETH, *map(str, args)]
    print("+", " ".join(command), flush=True)
    try:
        result = subprocess.run(
            command,
            env={**os.environ, "RUST_LOG": "off" if quiet else "info,sync=error"},
            capture_output=True,
            text=True,
            timeout=120,
        )
    except subprocess.TimeoutExpired as error:
        print((error.stdout or b"").decode(errors="replace"), end="", flush=True)
        print(
            (error.stderr or b"").decode(errors="replace"),
            end="", file=sys.stderr, flush=True,
        )
        raise
    if not quiet or result.returncode:
        print(result.stdout, end="", flush=True)
        print(result.stderr, end="", file=sys.stderr, flush=True)
    result.check_returncode()
    return result.stdout


def rpc(ipc, method, params=()):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(2)
        connection.connect(str(ipc))
        request = {"jsonrpc": "2.0", "id": 1, "method": method, "params": list(params)}
        connection.sendall(json.dumps(request).encode() + b"\n")
        with connection.makefile("r") as response:
            result = json.loads(response.readline())
    if "error" in result:
        raise RuntimeError(f"{method}: {result['error']}")
    return result["result"]


def smoke_test(root):
    source = ["--chain", FIXTURE / "genesis.json", "--datadir", root / "source"]
    target = ["--chain", FIXTURE / "genesis.json", "--datadir", root / "target"]
    # Both download stages, including tip retrieval, must use only the seeded peer.
    network = [
        "--disable-discovery", "--no-persist-peers", "--nat", "none",
        "--addr", "127.0.0.1", "--port", "0",
    ]
    run("import", *source, "--fail-on-invalid-block", FIXTURE / "chain.rlp")
    ipc = root / "peer.ipc"
    with (root / "peer.log").open("w") as log:
        peer = subprocess.Popen(
            [RETH, "node", *map(str, source), *network, "--max-outbound-peers", "0",
             "--log.file.directory", str(root / "logs"),
             "--authrpc.port", "0", "--ipcpath", str(ipc)],
            stdout=log,
            stderr=subprocess.STDOUT,
            env={**os.environ, "RUST_LOG": "info,sync=error"},
        )
        try:
            deadline = time.monotonic() + 30
            while True:
                if peer.poll() is not None:
                    raise RuntimeError(f"Fixture peer exited with {peer.returncode}")
                try:
                    info = rpc(ipc, "admin_nodeInfo")
                    break
                except (FileNotFoundError, ConnectionRefusedError, TimeoutError):
                    if time.monotonic() >= deadline:
                        raise TimeoutError(
                            "Fixture peer did not become ready within 30 seconds"
                        )
                    time.sleep(0.1)

            head = rpc(ipc, "eth_getBlockByNumber", [hex(TIP), False])
            forkchoice = json.loads((FIXTURE / "headfcu.json").read_text())["params"][0]
            expected_hash = forkchoice["headBlockHash"]
            assert head is not None and head["hash"] == expected_hash, head
            assert int(rpc(ipc, "eth_blockNumber"), 16) == TIP

            config = root / "target.toml"
            config.write_text(
                "[peers]\nconnect_trusted_nodes_only = true\n"
                f"trusted_nodes = [{json.dumps(info['enode'])}]\n"
            )
            # With storage v2, execution writes hashed state directly. Running the no-op
            # hashing stages would unwind that state without restoring it.
            for stage, checkpoint in STAGES.items():
                # The target is fresh: do not unwind stages that have never executed,
                # especially across the fixture's Cancun boundary.
                run(
                    "stage", "run", stage, *target, "--config", config, *network,
                    "--from", "0", "--to", TIP, "--batch-size", "8",
                    "--commit", "--checkpoints", "--skip-unwind",
                )
                saved = json.loads(run(
                    "db", *target, "get", "mdbx", "StageCheckpoints", checkpoint,
                    quiet=True,
                ))
                assert saved["block_number"] == TIP, (stage, saved)

            # Downloaded canonical headers live in static files, not MDBX.
            _, block_hash = run(
                "db", *target, "get", "static-file", "headers", TIP, quiet=True,
            ).split("\nBlockHash\n")
            actual_hash = json.loads(block_hash)
            assert actual_hash == expected_hash, actual_hash
            print(
                f"All {len(STAGES)} stages reached fixture block {TIP} "
                f"({expected_hash})."
            )
        finally:
            if peer.poll() is None:
                peer.terminate()
                try:
                    peer.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    peer.kill()
                    peer.wait(timeout=10)
            if sys.exc_info()[0] is not None:
                log.flush()
                print((root / "peer.log").read_text(), end="", flush=True)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="reth-stage-") as directory:
        smoke_test(Path(directory))
