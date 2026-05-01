#!/usr/bin/env python3

"""Extract Merkle-stage leaves and trie nodes from a reth trace log.

The trace logs emitted by `reth` only include hashed addresses and hashed storage
slots, so this script reports the hashed values exactly as they appear in the
trace.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


STAGE_RE = re.compile(r"stage=(Merkle[^}]+)")
STORAGE_TRIE_ADDR_RE = re.compile(r"storage_trie\{addr=(0x[0-9a-f]+)(?: [^}]*)?\}")
ACCOUNT_LEAF_RE = re.compile(
    r"Leaf\("
    r"(0x[0-9a-f]+), "
    r"Account \{ nonce: ([0-9]+), balance: ([0-9]+), bytecode_hash: "
    r"(?:None|Some\((0x[0-9a-f]+)\))"
    r" \}\)"
)
STORAGE_LEAF_RE = re.compile(r"Leaf\((0x[0-9a-f]+), (.+)\)\)\)$")
BRANCH_RE = re.compile(
    r"Branch\(TrieBranchNode \{ "
    r"key: Nibbles\((0x[0-9a-f]*)\), "
    r"value: (0x[0-9a-f]+), "
    r"children_are_in_trie: (true|false) "
    r"\}\)"
)
STATE_ROOT_RE = re.compile(
    r"calculated state root "
    r"root=(0x[0-9a-f]+) "
    r"duration=([^ ]+) "
    r"branches_added=([0-9]+) "
    r"leaves_added=([0-9]+)"
)
STORAGE_ROOT_RE = re.compile(
    r"calculated storage root "
    r"root=(0x[0-9a-f]+) "
    r"hashed_address=(0x[0-9a-f]+) "
    r"duration=([^ ]+) "
    r"branches_added=([0-9]+) "
    r"leaves_added=([0-9]+)"
)


def bool_from_string(value: str) -> bool:
    return value == "true"


def add_unique(bucket: dict[tuple[str, ...], dict[str, object]], key: tuple[str, ...], payload: dict[str, object], line_number: int) -> None:
    record = bucket.get(key)
    if record is None:
        bucket[key] = {
            **payload,
            "occurrences": 1,
            "first_line": line_number,
            "last_line": line_number,
        }
        return

    record["occurrences"] = int(record["occurrences"]) + 1
    record["last_line"] = line_number


def sorted_records(bucket: dict[tuple[str, ...], dict[str, object]]) -> list[dict[str, object]]:
    return sorted(bucket.values(), key=lambda item: (int(item["first_line"]), int(item["last_line"])))


def parse_trace(trace_path: Path) -> dict[str, object]:
    account_leaves: dict[tuple[str, ...], dict[str, object]] = {}
    storage_leaves: dict[tuple[str, ...], dict[str, object]] = {}
    state_trie_nodes: dict[tuple[str, ...], dict[str, object]] = {}
    storage_trie_nodes: dict[tuple[str, ...], dict[str, object]] = {}
    state_roots: list[dict[str, object]] = []
    storage_roots: list[dict[str, object]] = []
    stages_seen: set[str] = set()

    account_leaf_occurrences = 0
    storage_leaf_occurrences = 0
    state_trie_node_occurrences = 0
    storage_trie_node_occurrences = 0

    with trace_path.open("r", encoding="utf-8", errors="replace") as handle:
        for line_number, line in enumerate(handle, start=1):
            stage_match = STAGE_RE.search(line)
            if stage_match is None:
                continue

            stage = stage_match.group(1)
            if not stage.startswith("Merkle"):
                continue
            stages_seen.add(stage)

            storage_addr_match = STORAGE_TRIE_ADDR_RE.search(line)
            storage_addr = storage_addr_match.group(1) if storage_addr_match else None

            if "trie::state_root: calculated state root" in line:
                state_root_match = STATE_ROOT_RE.search(line)
                if state_root_match:
                    state_roots.append(
                        {
                            "line": line_number,
                            "stage": stage,
                            "root": state_root_match.group(1),
                            "duration": state_root_match.group(2),
                            "branches_added": int(state_root_match.group(3)),
                            "leaves_added": int(state_root_match.group(4)),
                        }
                    )
                continue

            if "trie::storage_root: calculated storage root" in line:
                storage_root_match = STORAGE_ROOT_RE.search(line)
                if storage_root_match:
                    storage_roots.append(
                        {
                            "line": line_number,
                            "stage": stage,
                            "hashed_address": storage_root_match.group(2),
                            "root": storage_root_match.group(1),
                            "duration": storage_root_match.group(3),
                            "branches_added": int(storage_root_match.group(4)),
                            "leaves_added": int(storage_root_match.group(5)),
                        }
                    )
                continue

            if "trie::node_iter: return=Ok(Some(" not in line:
                continue

            branch_match = BRANCH_RE.search(line)
            if branch_match:
                node_payload = {
                    "stage": stage,
                    "key": branch_match.group(1),
                    "node_hash": branch_match.group(2),
                    "children_are_in_trie": bool_from_string(branch_match.group(3)),
                }
                if "trie_type=Storage" in line and storage_addr is not None:
                    storage_trie_node_occurrences += 1
                    add_unique(
                        storage_trie_nodes,
                        (
                            stage,
                            storage_addr,
                            node_payload["key"],
                            node_payload["node_hash"],
                            str(node_payload["children_are_in_trie"]),
                        ),
                        {**node_payload, "hashed_address": storage_addr},
                        line_number,
                    )
                elif "trie_type=State" in line:
                    state_trie_node_occurrences += 1
                    add_unique(
                        state_trie_nodes,
                        (
                            stage,
                            node_payload["key"],
                            node_payload["node_hash"],
                            str(node_payload["children_are_in_trie"]),
                        ),
                        node_payload,
                        line_number,
                    )
                continue

            account_leaf_match = ACCOUNT_LEAF_RE.search(line)
            if account_leaf_match and "trie_type=State" in line:
                account_leaf_occurrences += 1
                bytecode_hash = account_leaf_match.group(4)
                add_unique(
                    account_leaves,
                    (
                        stage,
                        account_leaf_match.group(1),
                        account_leaf_match.group(2),
                        account_leaf_match.group(3),
                        bytecode_hash or "None",
                    ),
                    {
                        "stage": stage,
                        "hashed_address": account_leaf_match.group(1),
                        "account": {
                            "nonce": account_leaf_match.group(2),
                            "balance": account_leaf_match.group(3),
                            "bytecode_hash": bytecode_hash,
                        },
                    },
                    line_number,
                )
                continue

            storage_leaf_match = STORAGE_LEAF_RE.search(line)
            if storage_leaf_match and "trie_type=Storage" in line and storage_addr is not None:
                storage_leaf_occurrences += 1
                add_unique(
                    storage_leaves,
                    (
                        stage,
                        storage_addr,
                        storage_leaf_match.group(1),
                        storage_leaf_match.group(2),
                    ),
                    {
                        "stage": stage,
                        "hashed_address": storage_addr,
                        "hashed_slot": storage_leaf_match.group(1),
                        "value": storage_leaf_match.group(2),
                    },
                    line_number,
                )

    return {
        "trace_file": str(trace_path),
        "summary": {
            "stages": sorted(stages_seen),
            "account_leaves": {
                "unique": len(account_leaves),
                "occurrences": account_leaf_occurrences,
            },
            "storage_leaves": {
                "unique": len(storage_leaves),
                "occurrences": storage_leaf_occurrences,
            },
            "state_trie_nodes": {
                "unique": len(state_trie_nodes),
                "occurrences": state_trie_node_occurrences,
            },
            "storage_trie_nodes": {
                "unique": len(storage_trie_nodes),
                "occurrences": storage_trie_node_occurrences,
            },
            "state_roots": len(state_roots),
            "storage_roots": len(storage_roots),
        },
        "account_leaves": sorted_records(account_leaves),
        "storage_leaves": sorted_records(storage_leaves),
        "state_trie_nodes": sorted_records(state_trie_nodes),
        "storage_trie_nodes": sorted_records(storage_trie_nodes),
        "state_roots": state_roots,
        "storage_roots": storage_roots,
    }


def print_summary(result: dict[str, object]) -> None:
    summary = result["summary"]
    if not isinstance(summary, dict):
        raise TypeError("summary must be a dictionary")

    print(f"trace_file: {result['trace_file']}")
    print(f"stages: {', '.join(summary['stages'])}")
    print(
        "account_leaves: "
        f"unique={summary['account_leaves']['unique']} "
        f"occurrences={summary['account_leaves']['occurrences']}"
    )
    print(
        "storage_leaves: "
        f"unique={summary['storage_leaves']['unique']} "
        f"occurrences={summary['storage_leaves']['occurrences']}"
    )
    print(
        "state_trie_nodes: "
        f"unique={summary['state_trie_nodes']['unique']} "
        f"occurrences={summary['state_trie_nodes']['occurrences']}"
    )
    print(
        "storage_trie_nodes: "
        f"unique={summary['storage_trie_nodes']['unique']} "
        f"occurrences={summary['storage_trie_nodes']['occurrences']}"
    )
    print(f"state_roots: {summary['state_roots']}")
    print(f"storage_roots: {summary['storage_roots']}")


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Extract touched Merkle-stage account/storage leaves and trie nodes from a trace log.",
    )
    parser.add_argument("trace_file", type=Path, help="Path to the trace log file")
    parser.add_argument(
        "--summary",
        action="store_true",
        help="Print only high-level counts instead of the full JSON payload",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Write the extracted payload to a file instead of stdout",
    )
    return parser


def main() -> int:
    parser = build_arg_parser()
    args = parser.parse_args()

    if not args.trace_file.is_file():
        parser.error(f"trace file does not exist: {args.trace_file}")

    result = parse_trace(args.trace_file)

    if args.summary:
        if args.output is not None:
            with args.output.open("w", encoding="utf-8") as handle:
                original_stdout = sys.stdout
                try:
                    sys.stdout = handle
                    print_summary(result)
                finally:
                    sys.stdout = original_stdout
            return 0

        print_summary(result)
        return 0

    payload = json.dumps(result, indent=2, sort_keys=False)
    if args.output is not None:
        args.output.write_text(payload + "\n", encoding="utf-8")
        return 0

    print(payload)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
