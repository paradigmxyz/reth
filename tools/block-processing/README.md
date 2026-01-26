# Worker Concurrency Visualization

Visualize trie worker utilization during block execution.

## Quick Start

```bash
cd tools/worker-concurrency
./app.py --port 5050 --tempo-url http://localhost:3200
```

## Endpoints

### Direct query (for debugging/CLI)
```bash
curl "http://localhost:5050/trace/<trace_id>?bucket_ms=5"
```

Returns:
```json
{
  "trace_id": "abc123",
  "duration_ms": 156.2,
  "worker_types": {
    "account_workers": {
      "span_count": 48,
      "max_concurrent": 16,
      "utilization_pct": 72.3,
      "series": [
        {"time_ms": 0, "active": 0},
        {"time_ms": 5, "active": 4},
        {"time_ms": 10, "active": 12},
        ...
      ]
    },
    "storage_workers": { ... }
  }
}
```

### Grafana JSON Datasource

1. Install "JSON API" datasource plugin in Grafana
2. Add datasource pointing to `http://localhost:5050`
3. Create dashboard with Time Series panel
4. Configure query:
   - Target: `account_workers` or `storage_workers`
   - Additional JSON data: `{"traceId": "your-trace-id-here", "bucketMs": 5}`

## Environment Variables

- `TEMPO_URL`: Tempo API endpoint (default: `http://localhost:3200`)
- `PORT`: Service port (default: `5050`)

## How It Works

1. Fetches trace from Tempo by trace ID
2. Extracts spans matching "account worker" and "storage worker"
3. Computes active worker count at each point in time:
   - Creates events: `(span.start, +1)`, `(span.end, -1)`
   - Sorts by timestamp
   - Running sum gives active count
4. Returns as Grafana-compatible time series

## Example Chart

```
Active
Workers
   16 ┤                                              
      │        ┌────────┐                            
   12 ┤   ────┘        └────┐      ┌────┐           
      │  ╱                  └─────┘    └──  ← Account Workers
    8 ┤─╱                                            
      │      ┌──────────────────┐                    
    4 ┤─────┘                    └───────────  ← Storage Workers
      │                                              
    0 ┼────┬────┬────┬────┬────┬────┬────┬────► ms
         0   20   40   60   80  100  120  140
```
