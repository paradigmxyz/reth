"""Summarize cumulative diagnostic snapshots from one-writer benchmark logs."""

import json
import sys
from pathlib import Path


def summarize(path):
    snapshots = []
    for line in path.read_text().splitlines():
        if "MDBX_MINCORE_DIAG " in line:
            snapshots.append(json.loads(line.split("MDBX_MINCORE_DIAG ", 1)[1]))
    if not snapshots:
        raise ValueError(f"No residency-cache snapshots in {path}")
    d = snapshots[-1]
    hits = sum(d["hits"])
    queries = d["misses"] - d["errors"]
    assert d["probes"] == hits + d["misses"]
    assert hits == d["resident_hits"] + d["nonresident_hits"]
    assert queries == d["resident_misses"] + d["nonresident_misses"]
    assert queries == d["all_resident"] + d["none_resident"] + d["mixed"]
    assert d["misses"] == sum(d["ghost_hits"]) + d["ghost_misses"]
    assert d["completed_windows"] == queries == sum(d["usage_hist"])
    assert d["used_units"] == sum(i * n for i, n in enumerate(d["usage_hist"]))
    assert d["completed_units"] == d["queried_units"]
    assert sum(d["offset_hits"]) == hits
    assert d["untracked_hits"] == 0
    pct = lambda n, denominator: round(100 * n / denominator, 4) if denominator else None
    return {
        "log": str(path),
        "snapshots": len(snapshots),
        "probes": d["probes"],
        "syscalls": d["misses"],
        "errors": d["errors"],
        "hit_pct": pct(hits, d["probes"]),
        "hit_position_pct_of_probes": [pct(n, d["probes"]) for n in d["hits"]],
        "nonresident_decision_pct": pct(d["nonresident_hits"] + d["nonresident_misses"], d["probes"]),
        "requested_unit_resident_on_miss_pct": pct(d["resident_misses"], queries),
        "queried_os_pages": d["queried_os_pages"],
        "queried_resident_os_pages_pct": pct(d["resident_os_pages"], d["queried_os_pages"]),
        "all_resident_query_pct": pct(d["all_resident"], queries),
        "none_resident_query_pct": pct(d["none_resident"], queries),
        "mixed_query_pct": pct(d["mixed"], queries),
        "mean_unique_units_used_per_query": round(d["used_units"] / queries, 4) if queries else None,
        "query_utilization_pct": pct(d["used_units"], d["queried_units"]),
        "one_unit_only_query_pct": pct(d["usage_hist"][1], queries),
        "repeated_hit_pct_of_hits": pct(d["repeated_hits"], hits),
        "hit_offset_below_8_pct": pct(sum(d["offset_hits"][:8]), hits),
        "hit_offset_below_16_pct": pct(sum(d["offset_hits"][:16]), hits),
        "hit_offset_below_32_pct": pct(sum(d["offset_hits"][:32]), hits),
        "recent_4_evictions_match_pct_of_misses": pct(sum(d["ghost_hits"][:4]), d["misses"]),
        "recent_16_evictions_match_pct_of_misses": pct(sum(d["ghost_hits"]), d["misses"]),
        "evictions": d["evictions"],
        "invalidated_windows": d["invalidated_windows"],
        "clears": d["clears"],
        "raw": d,
    }


if __name__ == "__main__":
    print(json.dumps([summarize(Path(p)) for p in sys.argv[1:]], indent=2))
