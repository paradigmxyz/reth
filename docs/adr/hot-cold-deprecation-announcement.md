# Reth Hot/Cold Storage Migration - Operator Announcement

> **Draft for: Telegram, Discord, Slack**

---

## Short Version (Twitter/Telegram)

🚨 **Reth Storage Breaking Change - Action Required**

Starting **v1.11.0** (week of Feb 2nd), we're transitioning to a new hot/cold storage architecture. 

**Legacy MDBX-only storage will be deprecated Feb 9th and removed Feb 16th.**

⚠️ **No in-place migration** - you must either:
1. Download fresh snapshots
2. Perform a full resync

📦 Snapshots: [PLACEHOLDER_SNAPSHOT_URL]

We will be completing the migration before Glamsterdam (Feb 27th).

---

## Long Version (Discord/Slack Announcement)

### 📢 Important: Reth Storage Architecture Migration

Hey operators! We're making a significant change to Reth's storage layer that **requires action from all full node operators**.

#### What's Changing?

We're transitioning from legacy MDBX-only storage to a new **hot/cold storage architecture**:
- **Hot storage (MDBX):** Recent state and frequently accessed data  
- **Cold storage (Static Files):** Historical headers, transactions, receipts

This change reduces our maintenance burden and improves performance, but **breaks backwards compatibility**.

#### Timeline

| Date | Milestone |
|------|-----------|
| **Feb 2, 2026** | v1.11.0 released with hot/cold behind feature flag |
| **Feb 2, 2026** | Deprecation officially announced (this post) |
| **Feb 16, 2026** | Legacy MDBX support removed |
| **Feb 27, 2026** | Glamsterdam hard fork |

#### What You Need To Do

**⚠️ There is no in-place migration path.**

You have two options:

**Option 1: Download Snapshots (Recommended)**
```bash
# Download the latest snapshot
[PLACEHOLDER_SNAPSHOT_DOWNLOAD_COMMAND]

# Start reth with new storage
reth node --datadir /path/to/new/datadir
```

📦 **Mainnet Snapshot:** [PLACEHOLDER_MAINNET_SNAPSHOT_URL]  
📦 **Sepolia Snapshot:** [PLACEHOLDER_SEPOLIA_SNAPSHOT_URL]  
📦 **Holesky Snapshot:** [PLACEHOLDER_HOLESKY_SNAPSHOT_URL]

**Option 2: Full Resync**
```bash
# Remove old datadir and resync
rm -rf /path/to/old/datadir
reth node --datadir /path/to/new/datadir
```

#### Who Is Affected?

- ✅ **Full node operators** - Action required
- ❌ Light clients - Not affected
- ❌ Archive snapshot users - Not affected

#### Questions?

- 💬 Ask in #reth-support
- 🐛 Report issues: https://github.com/paradigmxyz/reth/issues

---

Thanks for running Reth! 🦀
