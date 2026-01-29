# Edge to MDBX Migration

This example demonstrates how to migrate data from an edge node (using RocksDB + static files) to a standard MDBX-based reth node.

## What is an Edge Node?

Edge nodes use the `--features edge` flag which enables alternative storage backends for better performance:
- `AccountsHistory` and `StoragesHistory` are stored in RocksDB instead of MDBX
- `AccountChangeSets` and `StorageChangeSets` are stored in static files instead of MDBX  
- `TransactionHashNumbers` are stored in RocksDB instead of MDBX

## What This Tool Migrates

This tool copies data from the edge storage locations to the standard MDBX tables:

| Data | From | To |
|------|------|-----|
| `AccountsHistory` | RocksDB | MDBX |
| `StoragesHistory` | RocksDB | MDBX |
| `AccountChangeSets` | Static Files | MDBX |
| `StorageChangeSets` | Static Files | MDBX |
| `TransactionHashNumbers` | RocksDB | MDBX |

## Usage

```bash
cargo run --example edge-to-mdbx-migration -- --datadir /path/to/edge/datadir
```

### Options

```
Options:
      --datadir <DATADIR>          Path to the reth datadir (must be an edge node)
      --skip-account-history       Skip migrating AccountsHistory [default: false]
      --skip-storage-history       Skip migrating StoragesHistory [default: false]
      --skip-account-changesets    Skip migrating AccountChangeSets [default: false]
      --skip-storage-changesets    Skip migrating StorageChangeSets [default: false]
      --skip-tx-hashes             Skip migrating TransactionHashNumbers [default: false]
      --dry-run                    Dry run - don't actually write to MDBX [default: false]
  -h, --help                       Print help
```

### Dry Run

Use `--dry-run` to preview what would be migrated without actually writing to MDBX:

```bash
cargo run --example edge-to-mdbx-migration -- --datadir /path/to/edge/datadir --dry-run
```

## Platform Support

This example only works on **Unix platforms** (Linux, macOS) because RocksDB support in reth is Unix-only.

## Batch Processing

The migration is done in batches to avoid OOM issues:
- History indices: 100,000 entries per batch
- Changesets: 10,000 blocks per batch  
- Transaction hashes: 500,000 entries per batch

## Notes

- The tool automatically detects which tables need migration based on the `StorageSettings` in the database
- Migration is additive - it doesn't delete the original data from RocksDB/static files
- After migration, you may want to update the `StorageSettings` to disable the edge storage options
