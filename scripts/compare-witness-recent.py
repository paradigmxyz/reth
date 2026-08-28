#!/usr/bin/env python3
"""Compare debug_executionWitness responses from local Reth and Geth."""

import argparse
import json
import shutil
import subprocess
import sys
import time
from urllib.error import URLError
from urllib.request import Request, urlopen


HEADER_FIELDS = (
    "parentHash",
    "sha3Uncles",
    "miner",
    "stateRoot",
    "transactionsRoot",
    "receiptsRoot",
    "logsBloom",
    "difficulty",
    "number",
    "gasLimit",
    "gasUsed",
    "timestamp",
    "extraData",
    "mixHash",
    "nonce",
    "baseFeePerGas",
    "withdrawalsRoot",
    "blobGasUsed",
    "excessBlobGas",
    "parentBeaconBlockRoot",
    "requestsHash",
)

QUANTITY_FIELDS = {
    "difficulty",
    "number",
    "gasLimit",
    "gasUsed",
    "timestamp",
    "baseFeePerGas",
    "blobGasUsed",
    "excessBlobGas",
}

RETH_WITNESS_MODE = "canonical"


def rpc(url, method, params, timeout):
    request = Request(
        url,
        data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urlopen(request, timeout=timeout) as response:
            payload = json.load(response)
    except (URLError, TimeoutError, json.JSONDecodeError) as error:
        raise RuntimeError(f"{url}: {method} failed: {error}") from error

    if "error" in payload:
        raise RuntimeError(f"{url}: {method} failed: {payload['error']}")
    return payload["result"]


def rlp(item):
    if isinstance(item, list):
        payload = b"".join(rlp(value) for value in item)
        offset = 0xC0
    else:
        payload = item
        if len(payload) == 1 and payload[0] < 0x80:
            return payload
        offset = 0x80

    if len(payload) <= 55:
        return bytes([offset + len(payload)]) + payload
    length = len(payload).to_bytes((len(payload).bit_length() + 7) // 8, "big")
    return bytes([offset + 55 + len(length)]) + length + payload


def value_bytes(value):
    if value is None:
        raise ValueError("missing header value")
    if not isinstance(value, str) or not value.startswith("0x"):
        raise ValueError(f"expected hex value, got {value!r}")
    hex_value = value[2:]
    if len(hex_value) % 2:
        hex_value = "0" + hex_value
    return bytes.fromhex(hex_value)


def geth_header_rlp(header):
    fields = []
    for field in HEADER_FIELDS:
        if field not in header or header[field] is None:
            break
        value = value_bytes(header[field])
        fields.append(value.lstrip(b"\x00") if field in QUANTITY_FIELDS else value)
    return "0x" + rlp(fields).hex()


def hex_set(witness, field):
    value = witness.get(field)
    if value is None:
        return set()
    if not isinstance(value, list) or not all(isinstance(entry, str) for entry in value):
        raise ValueError(f"{field} is not an array of hex strings")
    return {entry.lower() for entry in value}


def header_set(witness, client):
    headers = witness.get("headers") or []
    if client == "reth":
        if not all(isinstance(header, str) for header in headers):
            raise ValueError("Reth headers are not RLP hex strings")
        return {header.lower() for header in headers}
    if not all(isinstance(header, dict) for header in headers):
        raise ValueError("Geth headers are not objects")
    return {geth_header_rlp(header) for header in headers}


def report_difference(label, expected, actual, severity="ERROR"):
    missing = expected - actual
    extra = actual - expected
    if not missing and not extra:
        return False
    print(f"{severity}: {label} differs", file=sys.stderr)
    for name, values in (("missing from Reth", missing), ("extra in Reth", extra)):
        if values:
            print(f"  {name}: {len(values)}", file=sys.stderr)
            for value in sorted(values)[:10]:
                print(f"    {value}", file=sys.stderr)
    return True


def report_superset(label, required, actual):
    missing = required - actual
    extras = actual - required
    if extras:
        print(f"{label}: {len(extras)} extra item(s) in Reth")
    if not missing:
        return False
    print(f"ERROR: {label} has {len(missing)} item(s) missing from Reth", file=sys.stderr)
    for value in sorted(missing)[:10]:
        print(f"  {value}", file=sys.stderr)
    return True


def compare_block(reth_url, geth_url, block, timeout):
    try:
        block_id = hex(block)
        reth_block = rpc(reth_url, "eth_getBlockByNumber", [block_id, False], timeout)
        geth_block = rpc(geth_url, "eth_getBlockByNumber", [block_id, False], timeout)
        if not reth_block or not geth_block:
            raise RuntimeError(f"block {block_id} is unavailable on both nodes")
        if reth_block["hash"].lower() != geth_block["hash"].lower():
            raise RuntimeError(
                f"block {block_id} differs: Reth={reth_block['hash']}, Geth={geth_block['hash']}"
            )

        reth = rpc(reth_url, "debug_executionWitness", [block_id, RETH_WITNESS_MODE], timeout)
        geth = rpc(geth_url, "debug_executionWitness", [block_id], timeout)

        reth_state, geth_state = hex_set(reth, "state"), hex_set(geth, "state")
        reth_codes, geth_codes = hex_set(reth, "codes"), hex_set(geth, "codes")
        reth_headers, geth_headers = header_set(reth, "reth"), header_set(geth, "geth")
    except (RuntimeError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2

    print(f"block {block_id} ({reth_block['hash']})")
    print(f"Reth witness mode: {RETH_WITNESS_MODE}; Geth uses its default mode")
    print(
        "state: Reth=%d Geth=%d; codes: Reth=%d Geth=%d; headers: Reth=%d Geth=%d"
        % (len(reth_state), len(geth_state), len(reth_codes), len(geth_codes), len(reth_headers), len(geth_headers))
    )

    # Reth can include cache-derived trie nodes Geth does not return. The required invariant is
    # that every Geth state node is also returned by Reth.
    failed = report_superset("state nodes", geth_state, reth_state)
    # Geth exposes only its default (legacy) mode, which includes created bytecode that canonical
    # Reth intentionally omits. Report that difference without failing the state-node check.
    report_difference("code blobs", geth_codes, reth_codes, severity="INFO")
    failed |= report_difference("RLP headers", geth_headers, reth_headers)

    if geth.get("keys") is None:
        print("keys: skipped (Geth returns null for this field)")
    else:
        failed |= report_difference("keys", hex_set(geth, "keys"), hex_set(reth, "keys"))

    if failed:
        return 1
    print("OK: every Geth witness item is present in Reth (Reth extras are allowed).")
    return 0


def latest_blocks(args):
    if args.block:
        return [int(args.block, 0)]

    reth_head = int(rpc(args.reth_url, "eth_blockNumber", [], args.timeout), 16)
    geth_head = int(rpc(args.geth_url, "eth_blockNumber", [], args.timeout), 16)
    head = min(reth_head, geth_head)
    blocks = range(max(0, head - args.count + 1), head + 1)
    print(f"comparing blocks {hex(blocks.start)} through {hex(head)}")
    return blocks


def alert(message):
    print(f"ALERT: {message}", file=sys.stderr)
    if shutil.which("notify-send"):
        subprocess.run(
            ["notify-send", "--urgency=critical", "Execution witness comparison failed", message],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )


def main():
    parser = argparse.ArgumentParser(
        description="Continuously compare the 20 latest common debug_executionWitness responses from local Reth and Geth."
    )
    parser.add_argument("--reth-url", default="http://127.0.0.1:8545")
    parser.add_argument("--geth-url", default="http://127.0.0.1:8546")
    parser.add_argument("--block", help="compare only this block (decimal or 0x-prefixed hexadecimal)")
    parser.add_argument("--count", type=int, default=20, help="number of latest common blocks (default: 20)")
    parser.add_argument("--interval", type=float, default=12, help="minimum seconds between passes (default: 12)")
    parser.add_argument("--once", action="store_true", help="run one pass instead of watching")
    parser.add_argument("--timeout", type=float, default=600, help="RPC timeout in seconds (default: 600)")
    args = parser.parse_args()

    if args.count < 1:
        parser.error("--count must be positive")
    if args.interval <= 0:
        parser.error("--interval must be positive")

    while True:
        started = time.monotonic()
        try:
            blocks = latest_blocks(args)
        except (RuntimeError, ValueError) as error:
            alert(str(error))
            return 2

        for block in blocks:
            result = compare_block(args.reth_url, args.geth_url, block, args.timeout)
            if result:
                alert(f"comparison failed at {hex(block)}")
                return result

        if args.once:
            return 0
        time.sleep(max(0, args.interval - (time.monotonic() - started)))


if __name__ == "__main__":
    sys.exit(main())
