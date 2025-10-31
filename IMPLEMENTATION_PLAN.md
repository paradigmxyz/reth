# Implementation Plan: Reduce Overlay State Provider Spikes

## 🎯 Goal
Reduce 600-700ms overlay provider spikes to <200ms by addressing MerkleChangeSets checkpoint lag.

## 📋 Step-by-Step Implementation

### Step 1: Add MerkleChangeSets Metrics (Priority: CRITICAL)

This gives us visibility into WHY the checkpoint lags.

#### **File: `crates/stages/stages/src/stages/merkle_changesets.rs`**

**A. Add metrics struct at top of file:**

```rust
#[cfg(feature = "metrics")]
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};

#[cfg(feature = "metrics")]
#[derive(Clone, Metrics)]
#[metrics(scope = "stages.merkle_changesets")]
struct MerkleChangeSetsMetrics {
    /// Total execution duration per stage run
    execution_duration: Histogram,

    /// Number of blocks processed per execution
    blocks_per_execution: Histogram,

    /// Time to process one block (average)
    per_block_duration: Histogram,

    /// Current checkpoint block number
    checkpoint_block: Gauge,

    /// Checkpoint lag in blocks (tip - checkpoint)
    checkpoint_lag: Gauge,

    /// Number of stage executions
    execution_count: Counter,
}
```

**B. Add metrics field to MerkleChangeSets struct:**

```rust
#[derive(Debug, Clone)]
pub struct MerkleChangeSets {
    retention_blocks: u64,
    #[cfg(feature = "metrics")]
    metrics: MerkleChangeSetsMetrics,
}
```

**C. Update constructors:**

```rust
pub const fn new() -> Self {
    #[cfg(not(feature = "metrics"))]
    {
        Self { retention_blocks: 64 }
    }

    #[cfg(feature = "metrics")]
    {
        Self {
            retention_blocks: 64,
            metrics: MerkleChangeSetsMetrics::default(),
        }
    }
}

pub const fn with_retention_blocks(retention_blocks: u64) -> Self {
    #[cfg(not(feature = "metrics"))]
    {
        Self { retention_blocks }
    }

    #[cfg(feature = "metrics")]
    {
        Self {
            retention_blocks,
            metrics: MerkleChangeSetsMetrics::default(),
        }
    }
}
```

**D. Instrument execute() method:**

Find the `fn execute()` method around line 299 and add:

```rust
fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
    #[cfg(feature = "metrics")]
    let execution_start = std::time::Instant::now();

    #[cfg(feature = "metrics")]
    self.metrics.execution_count.increment(1);

    // ... existing code ...

    let target_range = self.determine_target_range(provider)?;
    let blocks_count = target_range.end.saturating_sub(target_range.start);

    #[cfg(feature = "metrics")]
    self.metrics.blocks_per_execution.record(blocks_count as f64);

    // ... existing population code ...

    // At the end, before returning:
    #[cfg(feature = "metrics")]
    {
        let execution_duration = execution_start.elapsed();
        self.metrics.execution_duration.record(execution_duration.as_secs_f64());

        if blocks_count > 0 {
            let per_block = execution_duration.as_secs_f64() / blocks_count as f64;
            self.metrics.per_block_duration.record(per_block);
        }

        // Record checkpoint state
        if let Some(checkpoint) = input.checkpoint {
            self.metrics.checkpoint_block.set(checkpoint.block_number as f64);

            // Calculate lag
            if let Ok(Some(tip)) = provider.last_finalized_block_number() {
                let lag = tip.saturating_sub(checkpoint.block_number);
                self.metrics.checkpoint_lag.set(lag as f64);
            }
        }
    }

    Ok(output)
}
```

---

### Step 2: Add Grafana Panels for New Metrics

#### **File: `dashboard.json`**

Add these panels after the existing "Overlay State Provider - Checkpoint Delta" panel:

```json
{
  "collapsed": false,
  "gridPos": {
    "h": 1,
    "w": 24,
    "x": 0,
    "y": 177
  },
  "id": 400,
  "panels": [],
  "title": "MerkleChangeSets Stage Performance",
  "type": "row"
},
{
  "datasource": {
    "type": "prometheus",
    "uid": "${datasource}"
  },
  "description": "Time to execute MerkleChangeSets stage. High values indicate stage is slow.",
  "fieldConfig": {
    "defaults": {
      "color": {
        "mode": "palette-classic"
      },
      "custom": {
        "axisBorderShow": false,
        "axisCenteredZero": false,
        "axisColorMode": "text",
        "axisLabel": "",
        "axisPlacement": "auto",
        "barAlignment": 0,
        "drawStyle": "line",
        "fillOpacity": 0,
        "gradientMode": "none",
        "hideFrom": {
          "legend": false,
          "tooltip": false,
          "viz": false
        },
        "lineInterpolation": "linear",
        "lineWidth": 1,
        "pointSize": 5,
        "scaleDistribution": {
          "type": "linear"
        },
        "showPoints": "auto",
        "spanNulls": false,
        "stacking": {
          "group": "A",
          "mode": "none"
        },
        "thresholdsStyle": {
          "mode": "off"
        }
      },
      "mappings": [],
      "thresholds": {
        "mode": "absolute",
        "steps": [
          {
            "color": "green",
            "value": 0
          },
          {
            "color": "yellow",
            "value": 1
          },
          {
            "color": "red",
            "value": 5
          }
        ]
      },
      "unit": "s"
    },
    "overrides": []
  },
  "gridPos": {
    "h": 8,
    "w": 12,
    "x": 0,
    "y": 178
  },
  "id": 401,
  "options": {
    "legend": {
      "calcs": [
        "mean",
        "max"
      ],
      "displayMode": "table",
      "placement": "bottom",
      "showLegend": true
    },
    "tooltip": {
      "mode": "multi",
      "sort": "none"
    }
  },
  "targets": [
    {
      "datasource": {
        "type": "prometheus",
        "uid": "${datasource}"
      },
      "expr": "reth_stages_merkle_changesets_execution_duration{$instance_label=\"$instance\",quantile=~\"(0.5|0.9|0.95|1)\"}",
      "legendFormat": "{{quantile}}",
      "refId": "A"
    }
  ],
  "title": "MerkleChangeSets - Execution Duration",
  "type": "timeseries"
},
{
  "datasource": {
    "type": "prometheus",
    "uid": "${datasource}"
  },
  "description": "Number of blocks processed per stage execution. High values mean infrequent updates causing checkpoint lag.",
  "fieldConfig": {
    "defaults": {
      "color": {
        "mode": "palette-classic"
      },
      "custom": {
        "axisBorderShow": false,
        "axisCenteredZero": false,
        "axisColorMode": "text",
        "axisLabel": "",
        "axisPlacement": "auto",
        "barAlignment": 0,
        "drawStyle": "line",
        "fillOpacity": 0,
        "gradientMode": "none",
        "hideFrom": {
          "legend": false,
          "tooltip": false,
          "viz": false
        },
        "lineInterpolation": "linear",
        "lineWidth": 1,
        "pointSize": 5,
        "scaleDistribution": {
          "type": "linear"
        },
        "showPoints": "auto",
        "spanNulls": false,
        "stacking": {
          "group": "A",
          "mode": "none"
        },
        "thresholdsStyle": {
          "mode": "off"
        }
      },
      "mappings": [],
      "thresholds": {
        "mode": "absolute",
        "steps": [
          {
            "color": "green",
            "value": 0
          },
          {
            "color": "yellow",
            "value": 50
          },
          {
            "color": "red",
            "value": 100
          }
        ]
      },
      "unit": "blocks"
    },
    "overrides": []
  },
  "gridPos": {
    "h": 8,
    "w": 12,
    "x": 12,
    "y": 178
  },
  "id": 402,
  "options": {
    "legend": {
      "calcs": [
        "mean",
        "max"
      ],
      "displayMode": "table",
      "placement": "bottom",
      "showLegend": true
    },
    "tooltip": {
      "mode": "multi",
      "sort": "none"
    }
  },
  "targets": [
    {
      "datasource": {
        "type": "prometheus",
        "uid": "${datasource}"
      },
      "expr": "reth_stages_merkle_changesets_blocks_per_execution{$instance_label=\"$instance\",quantile=~\"(0.5|0.9|0.95|1)\"}",
      "legendFormat": "{{quantile}}",
      "refId": "A"
    }
  ],
  "title": "MerkleChangeSets - Blocks Per Execution",
  "type": "timeseries"
}
```

---

### Step 3: Test and Baseline

**Commands to run:**

```bash
# 1. Format code
cargo +nightly fmt --all

# 2. Build with metrics
cargo build --release --features metrics

# 3. Run and collect metrics for 10 minutes
# Query Prometheus:
rate(reth_stages_merkle_changesets_execution_count[5m])
reth_stages_merkle_changesets_blocks_per_execution{quantile="0.5"}
reth_stages_merkle_changesets_checkpoint_lag
```

**Baseline expectations:**
- Execution frequency: Should see ~1-2 per minute
- Blocks per execution: Probably 50-70 (explains 45-74 block lag!)
- Checkpoint lag: Should match earlier observations (45-74 blocks)

---

### Step 4: Implement Fix (Reduce Batch Size)

Once you confirm batch size is too large:

#### **Find where MerkleChangeSets is constructed**

```bash
# Search for where stage is created
rg "MerkleChangeSets::new|MerkleChangeSets::with_retention_blocks" --type rust
```

Likely in `crates/node/builder/src/launch/` or similar pipeline setup.

#### **Change from:**

```rust
MerkleChangeSets::new()  // Default 64 blocks
```

#### **To:**

```rust
MerkleChangeSets::with_retention_blocks(20)  // Update every 20 blocks
```

**Or** add time-based triggering (more complex):

```rust
// In stage execution loop
if last_merkle_update.elapsed() > Duration::from_secs(10) {
    stage.execute(provider, input)?;
    last_merkle_update = Instant::now();
}
```

---

### Step 5: Validate Fix

**After deploying fix:**

```bash
# Check new metrics
reth_stages_merkle_changesets_blocks_per_execution{quantile="0.5"}
# Should be ~20 now (was 50-70)

reth_stages_merkle_changesets_checkpoint_lag
# Should be 0-20 blocks (was 45-74)

# Check overlay metrics
rate(reth_storage_overlay_state_provider_reverts_required[5m])
# Should stay ~1.7 req/s but...

# Check overlay duration
reth_storage_overlay_state_provider_trie_reverts_duration{quantile="0.9"}
# Should drop to ~100-200ms (was 500-600ms)

reth_storage_overlay_state_provider_total_database_provider_ro_duration{quantile="0.9"}
# Should drop to ~200-300ms (was 600-700ms)
```

**Success criteria:**
- ✅ Checkpoint lag: <25 blocks (was 45-74)
- ✅ Trie revert duration: <250ms (was 500-600ms)
- ✅ Total overlay duration: <350ms (was 600-700ms)
- ✅ No regression in overall throughput

---

### Step 6: Add Alerting

#### **File: `alerting_rules.yml` (or similar)**

```yaml
groups:
  - name: merkle_changesets
    interval: 30s
    rules:
      - alert: MerkleChangeSetsCheckpointLagging
        expr: |
          reth_stages_merkle_changesets_checkpoint_lag > 50
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "MerkleChangeSets checkpoint is {{ $value }} blocks behind"
          description: "Checkpoint lag >50 blocks. Target: <25 blocks. Check stage execution frequency."

      - alert: MerkleChangeSetsExecutionSlow
        expr: |
          reth_stages_merkle_changesets_execution_duration{quantile="0.9"} > 5
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "MerkleChangeSets taking {{ $value }}s to execute (p90)"
          description: "Stage execution >5s. Check DB performance or reduce batch size."

      - alert: OverlayRevertsSpiking
        expr: |
          reth_storage_overlay_state_provider_trie_reverts_duration{quantile="0.9"} > 0.5
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Overlay trie reverts taking {{ $value }}s (p90)"
          description: "Revert duration >500ms. Check MerkleChangeSets checkpoint lag."
```

---

## 🧪 Testing Checklist

### Before Fix
- [ ] Baseline checkpoint lag metric
- [ ] Baseline blocks per execution
- [ ] Baseline overlay revert duration
- [ ] Baseline Engine API throughput

### After Fix
- [ ] Checkpoint lag reduced to <25 blocks
- [ ] Overlay revert duration reduced to <250ms
- [ ] No throughput regression
- [ ] Alerts configured and tested

### Production Validation
- [ ] Deploy to staging first
- [ ] Run for 24 hours
- [ ] Compare metrics before/after
- [ ] Validate under peak load
- [ ] Deploy to production

---

## 📊 Expected Results

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| Checkpoint Lag | 45-74 blocks | 10-20 blocks | 60-70% ↓ |
| Trie Reverts | 500-600ms | 100-200ms | 65-80% ↓ |
| Total Overlay | 600-700ms | 200-300ms | 60-70% ↓ |
| Impact Rate | 1.72 req/s | 1.72 req/s | Same |
| State Root | 5-10μs | 5-10μs | Unchanged |

---

## 🚨 Rollback Plan

If fix causes issues:

1. **Revert retention_blocks change**:
   ```rust
   MerkleChangeSets::new()  // Back to default 64
   ```

2. **Monitor for recovery**:
   ```bash
   # Checkpoint lag should return to baseline
   reth_stages_merkle_changesets_checkpoint_lag
   ```

3. **Alternative approach**: Implement async revert fetching instead

---

## 📝 Next Steps

1. ✅ Add MerkleChangeSets metrics (Step 1)
2. ✅ Add Grafana panels (Step 2)
3. ✅ Collect baseline data (Step 3)
4. 🎯 Implement fix based on baseline (Step 4)
5. 📊 Validate results (Step 5)
6. 🚨 Configure alerts (Step 6)

**Estimated timeline**: 1-2 weeks

Want me to start implementing Step 1 (add the metrics)?
