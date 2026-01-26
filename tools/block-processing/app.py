#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["fastapi", "uvicorn", "httpx", "cachetools"]
# ///
"""
Worker Concurrency Visualization Service

Fetches trace from Tempo, computes active worker count over time.

Usage:
    ./app.py --port 5050 --tempo-url http://localhost:3200

Grafana setup:
    1. Install Infinity datasource plugin
    2. Add datasource pointing to http://localhost:5050
    3. Query /dashboard/{traceId} endpoint
"""

import asyncio
import os
from collections import defaultdict

import httpx
from cachetools import TTLCache
from fastapi import FastAPI, HTTPException

app = FastAPI(title="Worker Concurrency Visualizer")

TEMPO_URL = os.environ.get("TEMPO_URL", "http://localhost:3200")
HTTP_TIMEOUT = 30.0

# Cache for traces: max 100 entries, TTL of 5 minutes
_trace_cache: TTLCache[str, dict] = TTLCache(maxsize=100, ttl=300)
# Lock to prevent concurrent fetches of the same trace (thundering herd)
_trace_locks: dict[str, asyncio.Lock] = {}
_locks_lock = asyncio.Lock()


async def _get_trace_lock(trace_id: str) -> asyncio.Lock:
    """Get or create a lock for a specific trace_id."""
    async with _locks_lock:
        if trace_id not in _trace_locks:
            _trace_locks[trace_id] = asyncio.Lock()
        return _trace_locks[trace_id]


async def fetch_trace(trace_id: str) -> dict:
    """Fetch trace from Tempo API with caching."""
    # Check cache first (without lock for fast path)
    if trace_id in _trace_cache:
        return _trace_cache[trace_id]

    # Use per-trace lock to prevent thundering herd
    lock = await _get_trace_lock(trace_id)
    async with lock:
        # Double-check cache after acquiring lock
        if trace_id in _trace_cache:
            return _trace_cache[trace_id]

        async with httpx.AsyncClient(timeout=HTTP_TIMEOUT) as client:
            resp = await client.get(f"{TEMPO_URL}/api/traces/{trace_id}")
            resp.raise_for_status()
            trace = resp.json()
            _trace_cache[trace_id] = trace
            return trace


def extract_worker_spans(trace: dict) -> dict[str, list[tuple[int, int]]]:
    """
    Extract child spans of worker spans (actual work being done).
    A worker is "busy" when it has active child spans, not just when the worker span is open.
    Returns dict mapping worker type -> list of (start_ns, end_ns) tuples for child work spans.
    """
    # First pass: collect all spans and identify worker span IDs
    all_spans = {}  # spanId -> span
    worker_span_ids = {}  # spanId -> worker_type

    def collect_spans(batches):
        for batch in batches:
            # Support both scopeSpans and legacy instrumentationLibrarySpans
            scope_spans_list = batch.get("scopeSpans") or batch.get("instrumentationLibrarySpans") or []
            for scope_spans in scope_spans_list:
                for span in scope_spans.get("spans", []):
                    span_id = span.get("spanId")
                    if not span_id:
                        continue
                    all_spans[span_id] = span

                    name = span.get("name", "").lower()
                    if "account worker" in name:
                        worker_span_ids[span_id] = "account_workers"
                    elif "storage worker" in name:
                        worker_span_ids[span_id] = "storage_workers"
                    elif "prewarm worker" in name:
                        worker_span_ids[span_id] = "prewarm_workers"

    if "batches" in trace:
        collect_spans(trace["batches"])
    elif "resourceSpans" in trace:
        collect_spans(trace["resourceSpans"])

    # Second pass: find child spans of worker spans (the actual work)
    workers = defaultdict(list)
    
    # Exclude metric/logging spans that don't represent actual work
    excluded_spans = {"trie cursor metrics", "hashed cursor metrics"}
    
    for span in all_spans.values():
        parent_id = span.get("parentSpanId", "")
        if parent_id in worker_span_ids:
            name = span.get("name", "").lower()
            if name in excluded_spans:
                continue
            worker_type = worker_span_ids[parent_id]
            start_ns = int(span.get("startTimeUnixNano", 0))
            end_ns = int(span.get("endTimeUnixNano", 0))
            if start_ns and end_ns:
                workers[worker_type].append((start_ns, end_ns))

    return workers


def extract_block_number(trace: dict) -> int | None:
    """
    Extract block number from the on_new_payload span's block_num attribute.
    """
    def search_spans(batches):
        for batch in batches:
            for scope_spans in batch.get("scopeSpans", []):
                for span in scope_spans.get("spans", []):
                    name = span.get("name", "").lower()
                    if "on_new_payload" in name:
                        for attr in span.get("attributes", []):
                            key = attr.get("key", "")
                            if key == "block_num":
                                value = attr.get("value", {})
                                if "intValue" in value:
                                    return int(value["intValue"])
                                if "stringValue" in value:
                                    return int(value["stringValue"])
        return None

    if "batches" in trace:
        return search_spans(trace["batches"])
    elif "resourceSpans" in trace:
        return search_spans(trace["resourceSpans"])
    return None


def extract_tx_execution_spans(trace: dict) -> tuple[list[tuple[int, int]], int, list[dict]]:
    """
    Extract transaction execution spans to track execution progress.
    Returns tuple of (list of (start_ns, end_ns) tuples, total_gas_used, tx_details).
    """
    tx_spans = []
    raw_tx_data = []
    total_gas = 0
    
    def collect_spans(batches):
        nonlocal total_gas
        for batch in batches:
            for scope_spans in batch.get("scopeSpans", []):
                for span in scope_spans.get("spans", []):
                    name = span.get("name", "").lower()
                    # Look for transaction execution spans
                    if "execute" in name and "tx" in name:
                        start_ns = int(span.get("startTimeUnixNano", 0))
                        end_ns = int(span.get("endTimeUnixNano", 0))
                        if start_ns and end_ns:
                            tx_spans.append((start_ns, end_ns))
                        
                        # Extract tx details
                        tx_hash = None
                        gas_used = 0
                        for attr in span.get("attributes", []):
                            key = attr.get("key", "")
                            val = attr.get("value", {})
                            if key == "gas_used":
                                gas = val.get("intValue") or val.get("stringValue")
                                if gas:
                                    gas_used = int(gas)
                                    total_gas += gas_used
                            elif key == "tx_hash":
                                tx_hash = val.get("stringValue")
                        
                        if tx_hash:
                            duration_ms = (end_ns - start_ns) / 1_000_000
                            raw_tx_data.append({
                                "start_ns": start_ns,
                                "tx_hash": tx_hash,
                                "duration_ms": round(duration_ms, 3),
                                "gas_used": gas_used,
                                "type": "execute"
                            })

    if "batches" in trace:
        collect_spans(trace["batches"])
    elif "resourceSpans" in trace:
        collect_spans(trace["resourceSpans"])
    
    # Sort by start time to infer tx_index from execution order
    raw_tx_data.sort(key=lambda x: x["start_ns"])
    tx_details = []
    for i, tx in enumerate(raw_tx_data):
        tx_details.append({
            "tx_index": i,
            "tx_hash": tx["tx_hash"],
            "duration_ms": tx["duration_ms"],
            "gas_used": tx["gas_used"],
            "type": tx["type"]
        })
    
    # Sort by end time to get completion order
    tx_spans.sort(key=lambda x: x[1])
    return tx_spans, total_gas, tx_details


def extract_prewarm_spans(trace: dict) -> tuple[list[tuple[int, int]], list[dict]]:
    """
    Extract prewarm transaction spans.
    Returns tuple of (list of (start_ns, end_ns) tuples, prewarm_details).
    """
    prewarm_spans = []
    prewarm_details = []
    
    def collect_spans(batches):
        for batch in batches:
            for scope_spans in batch.get("scopeSpans", []):
                for span in scope_spans.get("spans", []):
                    name = span.get("name", "").lower()
                    # Look for prewarm tx spans (similar pattern to execute tx)
                    if "prewarm" in name and "tx" in name:
                        start_ns = int(span.get("startTimeUnixNano", 0))
                        end_ns = int(span.get("endTimeUnixNano", 0))
                        if start_ns and end_ns:
                            prewarm_spans.append((start_ns, end_ns))
                        
                        # Extract tx details
                        tx_hash = None
                        is_success = None
                        gas_used = None
                        for attr in span.get("attributes", []):
                            if attr.get("key") == "tx_hash":
                                tx_hash = attr.get("value", {}).get("stringValue")
                            elif attr.get("key") == "is_success":
                                is_success = attr.get("value", {}).get("boolValue")
                            elif attr.get("key") == "gas_used":
                                val = attr.get("value", {})
                                gas_used = val.get("intValue") or val.get("stringValue")
                        
                        if tx_hash:
                            duration_ms = (end_ns - start_ns) / 1_000_000
                            prewarm_details.append({
                                "tx_hash": tx_hash,
                                "duration_ms": round(duration_ms, 3),
                                "type": "prewarm",
                                "is_success": is_success,
                                "gas_used": int(gas_used) if gas_used else None,
                            })

    if "batches" in trace:
        collect_spans(trace["batches"])
    elif "resourceSpans" in trace:
        collect_spans(trace["resourceSpans"])
    
    prewarm_spans.sort(key=lambda x: x[1])
    return prewarm_spans, prewarm_details


def extract_main_phases(trace: dict) -> dict[str, tuple[int, int]]:
    """
    Extract the three main processing phases:
    - spawn_payload_processor
    - execution  
    - await_state_root
    Returns dict mapping phase name -> (start_ns, end_ns).
    """
    phases = {}

    # Use lowercase keys for case-insensitive matching
    phase_mapping = {
        "spawn_payload_processor": "spawn",
        "execution": "execution",
        "await_state_root": "state_root",
    }

    def collect_spans(batches):
        for batch in batches:
            scope_spans_list = batch.get("scopeSpans") or batch.get("instrumentationLibrarySpans") or []
            for scope_spans in scope_spans_list:
                for span in scope_spans.get("spans", []):
                    name = span.get("name", "").lower()

                    if name in phase_mapping:
                        phase_key = phase_mapping[name]
                        start_ns = int(span.get("startTimeUnixNano", 0))
                        end_ns = int(span.get("endTimeUnixNano", 0))
                        if start_ns and end_ns:
                            phases[phase_key] = (start_ns, end_ns)

    if "batches" in trace:
        collect_spans(trace["batches"])
    elif "resourceSpans" in trace:
        collect_spans(trace["resourceSpans"])

    return phases


def compute_execution_progress(
    tx_spans: list[tuple[int, int]],
    min_time_ns: int,
) -> list[dict]:
    """
    Compute cumulative transaction execution progress using event-based approach.
    Returns series of {time_ms, completed} at each completion point.
    """
    if not tx_spans:
        return []

    # Get sorted end times and emit a point at each completion
    end_times = sorted(span[1] for span in tx_spans)
    
    series = [{"time_ms": 0.0, "completed": 0}]
    prev_time_ms = 0.0
    for i, end_ns in enumerate(end_times):
        time_ms = round((end_ns - min_time_ns) / 1_000_000, 3)
        # Ensure strictly ascending by adding small offset for duplicates
        if time_ms <= prev_time_ms:
            time_ms = round(prev_time_ms + 0.001, 3)
        series.append({"time_ms": time_ms, "completed": i + 1})
        prev_time_ms = time_ms

    return series


def compute_concurrency_series(
    spans: list[tuple[int, int]],
    min_time_ns: int,
) -> list[dict]:
    """
    Compute active worker count using event-based approach.
    Returns series of {time_ms, active} at each transition point.
    """
    if not spans:
        return []

    # Build events: +1 at start, -1 at end
    events: list[tuple[int, int]] = []
    for start_ns, end_ns in spans:
        events.append((start_ns, 1))
        events.append((end_ns, -1))
    
    # Sort by time, with -1 before +1 at same time (end before start)
    events.sort(key=lambda e: (e[0], e[1]))

    # Emit a point at each transition, ensuring strictly ascending times
    series = [{"time_ms": 0.0, "active": 0}]
    active = 0
    prev_time_ms = 0.0
    
    for time_ns, delta in events:
        time_ms = round((time_ns - min_time_ns) / 1_000_000, 3)
        active += delta
        # Only emit if value changed
        if series[-1]["active"] != active:
            # Ensure strictly ascending
            if time_ms <= prev_time_ms:
                time_ms = round(prev_time_ms + 0.001, 3)
            series.append({"time_ms": time_ms, "active": active})
            prev_time_ms = time_ms

    return series


@app.get("/")
async def health():
    return {"status": "ok"}


@app.post("/cache/clear")
async def clear_cache():
    """Clear the trace cache."""
    _trace_cache.clear()
    return {"status": "ok", "message": "Cache cleared"}


@app.get("/traces")
async def list_traces(limit: int = 50):
    """
    List recent traces with on_new_payload spans from Tempo.
    Returns trace metadata for use in a selection table.
    """
    import time

    # Search for traces with on_new_payload span in last 24h
    end = int(time.time())
    start = end - 86400  # 24 hours ago

    try:
        async with httpx.AsyncClient(timeout=HTTP_TIMEOUT) as client:
            # Use TraceQL to find traces with on_new_payload spans
            resp = await client.get(
                f"{TEMPO_URL}/api/search",
                params={
                    "q": '{ name =~ ".*on_new_payload.*" }',
                    "limit": limit,
                    "start": start,
                    "end": end,
                },
            )
            resp.raise_for_status()
            search_result = resp.json()
    except httpx.HTTPError as e:
        raise HTTPException(status_code=500, detail=f"Failed to search traces: {e}")

    traces = []
    for trace_meta in search_result.get("traces", []):
        trace_id = trace_meta.get("traceID", "")
        root_name = trace_meta.get("rootServiceName", "")
        root_trace = trace_meta.get("rootTraceName", "")
        duration_ms = trace_meta.get("durationMs", 0)
        start_time = trace_meta.get("startTimeUnixNano", 0)

        # Try to fetch block number from the trace
        block_number = None
        try:
            trace = await fetch_trace(trace_id)
            block_number = extract_block_number(trace)
        except Exception:
            pass

        traces.append({
            "trace_id": trace_id,
            "block_number": block_number,
            "duration_ms": duration_ms,
            "root_service": root_name,
            "root_trace": root_trace,
            "start_time_ns": start_time,
        })

    # Sort by block number descending (most recent first)
    traces.sort(key=lambda x: x.get("block_number") or 0, reverse=True)

    return traces


@app.get("/dashboard/{trace_id}")
async def dashboard_data(trace_id: str):
    """
    Returns all data needed for a complete dashboard in one request.
    Grafana Infinity can extract different parts for different panels.
    """
    try:
        trace = await fetch_trace(trace_id)
    except httpx.HTTPError as e:
        raise HTTPException(status_code=500, detail=f"Failed to fetch trace: {e}")

    worker_spans = extract_worker_spans(trace)
    block_number = extract_block_number(trace)
    tx_spans, total_gas, tx_details = extract_tx_execution_spans(trace)
    prewarm_spans, prewarm_details = extract_prewarm_spans(trace)
    main_phases = extract_main_phases(trace)

    # Compute time range from all available spans (worker spans + phases)
    all_times: list[int] = []
    for spans in worker_spans.values():
        for start, end in spans:
            all_times.extend([start, end])
    for start, end in main_phases.values():
        all_times.extend([start, end])
    
    if not all_times:
        raise HTTPException(status_code=404, detail="No spans found")
        
    min_time_ns = min(all_times)
    max_time_ns = max(all_times)
    duration_ms = (max_time_ns - min_time_ns) / 1_000_000

    result = {
        "trace_id": trace_id,
        "block_number": block_number,
        "duration_ms": round(duration_ms, 2),
        "total_txs": len(tx_spans),
        "mgas_per_second": round(total_gas / (duration_ms / 1000) / 1_000_000, 2) if duration_ms > 0 else 0,
        "series": {},
    }

    for worker_type, spans in worker_spans.items():
        series = compute_concurrency_series(spans, min_time_ns)
        points = [{"time": p["time_ms"], "active": p["active"]} for p in series]
        # Add final point at trace end for consistent X axis
        if points and points[-1]["time"] < duration_ms:
            points.append({"time": round(duration_ms, 3), "active": 0})
        result["series"][worker_type] = points

    # Add execution progress series
    if tx_spans:
        progress_series = compute_execution_progress(tx_spans, min_time_ns)
        points = [{"time": p["time_ms"], "completed": p["completed"]} for p in progress_series]
        # Add final point at trace end
        if points and points[-1]["time"] < duration_ms:
            points.append({"time": round(duration_ms, 3), "completed": points[-1]["completed"]})
        result["series"]["execution_progress"] = points

    # Add prewarm progress series
    if prewarm_spans:
        prewarm_progress = compute_execution_progress(prewarm_spans, min_time_ns)
        points = [{"time": p["time_ms"], "completed": p["completed"]} for p in prewarm_progress]
        # Add final point at trace end
        if points and points[-1]["time"] < duration_ms:
            points.append({"time": round(duration_ms, 3), "completed": points[-1]["completed"]})
        result["series"]["prewarm_progress"] = points

    # Add main phases for horizontal bar visualization
    if main_phases:
        # Single row format for stacked bar chart
        phases_row = {"category": "Processing"}
        for phase_name in ["spawn", "execution", "state_root"]:
            if phase_name in main_phases:
                start_ns, end_ns = main_phases[phase_name]
                phases_row[phase_name] = round((end_ns - start_ns) / 1_000_000, 2)
        result["phases"] = [phases_row]

    # Merge tx details with prewarm details, matching by tx_hash
    tx_map = {tx["tx_hash"]: tx.copy() for tx in tx_details}
    for prewarm in prewarm_details:
        tx_hash = prewarm["tx_hash"]
        is_success = prewarm.get("is_success")
        if is_success is True:
            status = "✓"
        else:
            status = "✗"
        
        if tx_hash in tx_map:
            tx_map[tx_hash]["prewarm_ms"] = prewarm["duration_ms"]
            tx_map[tx_hash]["prewarm_status"] = status
        else:
            tx_map[tx_hash] = {
                "tx_index": None,
                "tx_hash": tx_hash,
                "duration_ms": 0,
                "gas_used": 0,
                "prewarm_ms": prewarm["duration_ms"],
                "prewarm_status": status,
            }
    
    # Sort by execution duration descending
    result["transactions"] = sorted(
        tx_map.values(),
        key=lambda x: x.get("duration_ms", 0),
        reverse=True
    )

    return result

if __name__ == "__main__":
    import argparse

    import uvicorn

    parser = argparse.ArgumentParser(description="Block processing dashboard server")
    parser.add_argument("--port", type=int, default=int(os.environ.get("PORT", 5050)), help="Port to listen on")
    parser.add_argument("--host", default="0.0.0.0", help="Host to bind to")
    parser.add_argument("--tempo-url", default=None, help="Tempo API URL (overrides TEMPO_URL env)")
    args = parser.parse_args()

    if args.tempo_url:
        TEMPO_URL = args.tempo_url

    uvicorn.run(app, host=args.host, port=args.port)
