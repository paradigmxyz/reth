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


def run(*args, quiet=False, expected_error=None):
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
    if expected_error is not None:
        assert result.returncode == 1, (result.returncode, result.stdout, result.stderr)
        assert result.stderr.splitlines()[:1] == [f"Error: {expected_error}"], result.stderr
        return result.stdout
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
                    "--commit", "--skip-unwind",
                    # Execution must checkpoint its state even without --checkpoints.
                    *([] if stage == "execution" else ["--checkpoints"]),
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


def test_legacy_execution_replay(root):
    config = root / "prune.toml"
    config.write_text(
        "[prune.segments]\nreceipts = { before = 1 }\n[stages.execution]\nmax_blocks = 1\n"
    )
    target = [
        "--chain", FIXTURE / "genesis.json", "--datadir", root / "legacy",
        "--storage.v2=false", "--config", config,
    ]
    run("import", *target, "--fail-on-invalid-block", FIXTURE / "chain.rlp")
    run(
        "stage", "dump", *target, "execution", "--from", TIP - 2, "--to", TIP - 1,
        "--output-datadir", root / "execution-dump",
    )

    def checkpoint():
        return json.loads(run(
            "db", *target, "get", "mdbx", "StageCheckpoints", "Execution", quiet=True,
        ))["block_number"]

    def plain_state():
        return [
            json.loads(run("db", *target, "list", table, "--json", "--len", "1000", quiet=True))
            for table in ["PlainAccountState", "PlainStorageState"]
        ]

    def run_range(start, end):
        run(
            "stage", "run", "execution", *target, "--from", start, "--to", end,
            "--batch-size", "1", "--commit",
        )
        assert checkpoint() == end

    run_range(TIP - 3, TIP - 2)
    expected = plain_state()
    for start, flags in [
        (TIP - 1, []), (TIP - 1, ["--skip-unwind"]), (TIP - 3, ["--skip-unwind"]),
    ]:
        run(
            "stage", "run", "execution", *target, "--from", start, "--to", TIP, "--commit",
            *flags, quiet=True,
            expected_error=(
                f"cannot start Execution at block {start}: stage checkpoint is at block {TIP - 2}"
            ),
        )
    assert checkpoint() == TIP - 2 and plain_state() == expected
    run_range(TIP - 2, TIP)
    # Older diagnostic runs could leave execution state ahead of the saved checkpoint.
    run(
        "db", *target, "stage-checkpoints", "set", "--stage", "execution",
        "--block-number", TIP - 2, "--clear-stage-unit",
    )
    run("stage", "run", "execution", *target, "--from", TIP - 2, "--to", TIP, "--commit")
    assert plain_state() != expected
    # A pipeline unwind must also include old writes when the saved checkpoint is below its target.
    run(
        "db", *target, "stage-checkpoints", "set", "--stage", "execution",
        "--block-number", TIP - 3, "--clear-stage-unit",
    )
    run("stage", "unwind", *target, "to-block", TIP - 2)
    assert plain_state() == expected
    assert checkpoint() == TIP - 2


def test_legacy_static_receipt_unwind(root):
    target = [
        "--chain", FIXTURE / "genesis.json", "--datadir", root / "archive", "--storage.v2=false",
    ]
    run("import", *target, "--fail-on-invalid-block", FIXTURE / "chain.rlp")
    run(
        "db", *target, "stage-checkpoints", "set", "--stage", "execution",
        "--block-number", TIP - 2, "--clear-stage-unit",
    )
    for height in [TIP - 1, TIP]:
        run(
            "stage", "unwind", *target, "to-block", height, quiet=True,
            expected_error=f"not able to find Receipts static file for block number {TIP - 1}",
        )
    saved = json.loads(run(
        "db", *target, "get", "mdbx", "StageCheckpoints", "Execution", quiet=True,
    ))
    assert saved["block_number"] == TIP - 2, saved
    # An unreachable target must not leave an unwind intent that blocks the valid retry.
    metadata = json.loads(run(
        "db", *target, "list", "Metadata", "--json", "--len", "100", quiet=True,
    ))
    assert "pipeline_unwind" not in dict(metadata)
    run("stage", "unwind", *target, "to-block", TIP - 2)
    run("stage", "run", "execution", *target, "--from", TIP - 3, "--to", TIP - 2, "--commit")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="reth-stage-") as directory:
        smoke_test(Path(directory))
        test_legacy_execution_replay(Path(directory))
        test_legacy_static_receipt_unwind(Path(directory))
