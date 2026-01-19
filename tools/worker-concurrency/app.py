#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["fastapi", "uvicorn", "httpx"]
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

import os
from collections import defaultdict

import httpx
from fastapi import FastAPI, HTTPException

app = FastAPI(title="Worker Concurrency Visualizer")

TEMPO_URL = os.environ.get("TEMPO_URL", "http://localhost:3200")
HTTP_TIMEOUT = 30.0


async def fetch_trace(trace_id: str) -> dict:
    """Fetch trace from Tempo API."""
    async with httpx.AsyncClient(timeout=HTTP_TIMEOUT) as client:
        resp = await client.get(f"{TEMPO_URL}/api/traces/{trace_id}")
        resp.raise_for_status()
        return resp.json()


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
    
    for span in all_spans.values():
        parent_id = span.get("parentSpanId", "")
        if parent_id in worker_span_ids:
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


def extract_tx_execution_spans(trace: dict) -> tuple[list[tuple[int, int]], int]:
    """
    Extract transaction execution spans to track execution progress.
    Returns tuple of (list of (start_ns, end_ns) tuples, total_gas_used).
    """
    tx_spans = []
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
                        # Extract gas_used attribute
                        for attr in span.get("attributes", []):
                            if attr.get("key") == "gas_used":
                                val = attr.get("value", {})
                                gas = val.get("intValue") or val.get("stringValue")
                                if gas:
                                    total_gas += int(gas)

    if "batches" in trace:
        collect_spans(trace["batches"])
    elif "resourceSpans" in trace:
        collect_spans(trace["resourceSpans"])
    
    # Sort by end time to get completion order
    tx_spans.sort(key=lambda x: x[1])
    return tx_spans, total_gas


def extract_prewarm_spans(trace: dict) -> list[tuple[int, int]]:
    """
    Extract prewarm transaction spans.
    Returns list of (start_ns, end_ns) tuples for each prewarmed transaction.
    """
    prewarm_spans = []
    
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

    if "batches" in trace:
        collect_spans(trace["batches"])
    elif "resourceSpans" in trace:
        collect_spans(trace["resourceSpans"])
    
    prewarm_spans.sort(key=lambda x: x[1])
    return prewarm_spans


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
    max_time_ns: int,
    bucket_ms: int = 1,
) -> list[dict]:
    """
    Compute cumulative transaction execution progress over time.
    Returns series of {time_ms, completed} showing how many txs completed at each point.
    """
    if not tx_spans:
        return []

    num_buckets = (max_time_ns - min_time_ns) // 1_000_000 + 1
    series = []

    # Get sorted end times for cumulative count - O(N log N)
    end_times = sorted(span[1] for span in tx_spans)

    # Use moving index for O(T + N) instead of O(T * N)
    completed_idx = 0
    for t in range(0, num_buckets, bucket_ms):
        sample_ns = min_time_ns + t * 1_000_000
        while completed_idx < len(end_times) and end_times[completed_idx] <= sample_ns:
            completed_idx += 1
        series.append({"time_ms": t, "completed": completed_idx})

    return series


def compute_concurrency_series(
    spans: list[tuple[int, int]],
    min_time_ns: int,
    max_time_ns: int,
    bucket_ms: int = 1,
) -> list[dict]:
    """
    Compute active worker count over time using sweep-line algorithm.
    O(N log N + T) instead of O(T * N).
    """
    if not spans:
        return []

    num_buckets = (max_time_ns - min_time_ns) // 1_000_000 + 1

    # Build difference array: +1 at start bucket, -1 at end bucket
    # This gives us O(N log N + T) complexity
    deltas: dict[int, int] = defaultdict(int)
    for start_ns, end_ns in spans:
        start_bucket = (start_ns - min_time_ns) // 1_000_000
        end_bucket = (end_ns - min_time_ns) // 1_000_000
        # Clamp to valid range
        start_bucket = max(0, start_bucket)
        end_bucket = min(num_buckets, end_bucket)
        deltas[start_bucket] += 1
        deltas[end_bucket] -= 1

    # Prefix sum to get active count at each bucket
    series = []
    active = 0
    for t in range(0, num_buckets, bucket_ms):
        active += deltas.get(t, 0)
        # For bucket_ms > 1, we need to accumulate intermediate deltas
        if bucket_ms > 1:
            for i in range(1, bucket_ms):
                if t + i < num_buckets:
                    active += deltas.get(t + i, 0)
        series.append({"time_ms": t, "active": active})

    return series


@app.get("/")
async def health():
    return {"status": "ok"}


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
    tx_spans, total_gas = extract_tx_execution_spans(trace)
    prewarm_spans = extract_prewarm_spans(trace)
    main_phases = extract_main_phases(trace)

    if not worker_spans:
        raise HTTPException(status_code=404, detail="No worker spans found")

    all_spans = [s for spans in worker_spans.values() for s in spans]
    if not all_spans:
        raise HTTPException(status_code=404, detail="No work spans found")
        
    min_time_ns = min(s[0] for s in all_spans)
    max_time_ns = max(s[1] for s in all_spans)
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
        series = compute_concurrency_series(spans, min_time_ns, max_time_ns, 1)
        result["series"][worker_type] = [
            {"time": p["time_ms"], "active": p["active"]}
            for p in series
        ]

    # Add execution progress series
    if tx_spans:
        progress_series = compute_execution_progress(tx_spans, min_time_ns, max_time_ns, 1)
        result["series"]["execution_progress"] = [
            {"time": p["time_ms"], "completed": p["completed"]}
            for p in progress_series
        ]

    # Add prewarm progress series
    if prewarm_spans:
        prewarm_progress = compute_execution_progress(prewarm_spans, min_time_ns, max_time_ns, 1)
        result["series"]["prewarm_progress"] = [
            {"time": p["time_ms"], "completed": p["completed"]}
            for p in prewarm_progress
        ]

    # Add main phases for horizontal bar visualization
    if main_phases:
        # Single row format for stacked bar chart
        phases_row = {"category": "Processing"}
        for phase_name in ["spawn", "execution", "state_root"]:
            if phase_name in main_phases:
                start_ns, end_ns = main_phases[phase_name]
                phases_row[phase_name] = round((end_ns - start_ns) / 1_000_000, 2)
        result["phases"] = [phases_row]

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
