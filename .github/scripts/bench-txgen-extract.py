#!/usr/bin/env python3
"""Extract raw blocks for txgen send-blocks.

This intentionally uses a numeric `debug_getRawBlock` parameter because some
public RPC providers reject the hex quantity form that alloy's Debug API emits.
"""

import argparse
import json
import sys
import time
import urllib.error
import urllib.request


def rpc_call(url: str, method: str, params: list, retries: int = 12):
    payload = json.dumps({"jsonrpc": "2.0", "method": method, "params": params, "id": 1}).encode()
    last_err = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(
                url,
                data=payload,
                headers={"Content-Type": "application/json", "User-Agent": "reth-bench-txgen"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=30) as resp:
                data = json.loads(resp.read())
            if "error" in data:
                raise RuntimeError(data["error"].get("message", data["error"]))
            return data["result"]
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, RuntimeError) as err:
            last_err = err
            if attempt + 1 == retries:
                break
            time.sleep(min(10.0, 0.5 * (2**attempt)))
    raise RuntimeError(last_err)


def parse_quantity(value: str) -> int:
    return int(value, 16) if isinstance(value, str) and value.startswith("0x") else int(value)


def validate_top_level_rlp(raw: str) -> None:
    if not isinstance(raw, str) or not raw.startswith("0x"):
        raise ValueError("raw block is not a hex string")
    data = bytes.fromhex(raw[2:])
    if not data:
        raise ValueError("raw block is empty")

    prefix = data[0]
    if prefix <= 0x7F:
        total = 1
    elif prefix <= 0xB7:
        total = 1 + prefix - 0x80
    elif prefix <= 0xBF:
        len_len = prefix - 0xB7
        total = 1 + len_len + int.from_bytes(data[1 : 1 + len_len], "big")
    elif prefix <= 0xF7:
        total = 1 + prefix - 0xC0
    else:
        len_len = prefix - 0xF7
        total = 1 + len_len + int.from_bytes(data[1 : 1 + len_len], "big")

    if total != len(data):
        raise ValueError(f"raw block RLP length mismatch: expected {total} bytes, got {len(data)}")


def fetch_raw_block(url: str, number: int, retries: int = 12) -> str:
    last_err = None
    for attempt in range(retries):
        try:
            raw = rpc_call(url, "debug_getRawBlock", [number], retries=1)
            validate_top_level_rlp(raw)
            return raw
        except (RuntimeError, ValueError) as err:
            last_err = err
            if attempt + 1 == retries:
                break
            time.sleep(min(10.0, 0.5 * (2**attempt)))
    raise RuntimeError(last_err)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rpc", required=True)
    parser.add_argument("--metadata-rpc")
    parser.add_argument("--from", dest="from_block", type=int, required=True)
    parser.add_argument("--to", dest="to_block", type=int, required=True)
    parser.add_argument("-o", "--output", required=True)
    args = parser.parse_args()

    if args.from_block > args.to_block:
        parser.error("--from must be <= --to")

    metadata_rpc = args.metadata_rpc or args.rpc
    total = args.to_block - args.from_block + 1
    started = time.monotonic()
    last_log = started

    with open(args.output, "w", encoding="utf-8") as out:
        for idx, number in enumerate(range(args.from_block, args.to_block + 1), start=1):
            try:
                raw = fetch_raw_block(args.rpc, number)
                block = rpc_call(metadata_rpc, "eth_getBlockByNumber", [hex(number), False])
            except RuntimeError as err:
                print(f"failed to fetch block {number}: {err}", file=sys.stderr)
                return 1

            if not raw or not block:
                print(f"missing block {number}", file=sys.stderr)
                return 1

            line = {
                "raw": raw,
                "key": block["hash"],
                "number": parse_quantity(block["number"]),
                "timestamp": parse_quantity(block["timestamp"]),
                "gas_used": parse_quantity(block["gasUsed"]),
                "gas_limit": parse_quantity(block["gasLimit"]),
                "tx_count": len(block.get("transactions", [])),
            }
            out.write(json.dumps(line, separators=(",", ":")) + "\n")

            now = time.monotonic()
            if idx == total or idx % 100 == 0 or now - last_log >= 5:
                elapsed = now - started
                rate = idx / elapsed if elapsed else 0
                print(f"extracted {idx}/{total} blocks ({idx / total * 100:.1f}%) - {rate:.0f} blocks/s", file=sys.stderr)
                last_log = now

    print(f"wrote {total} blocks to {args.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
