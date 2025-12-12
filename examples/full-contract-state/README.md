# Full Contract State Example

This example demonstrates how to extract the complete state of a specific contract from the reth database.

## What it does

The example shows how to:

1. **Connect to a reth database** - Uses the recommended builder pattern to create a read-only provider
2. **Get basic account information** - Retrieves balance, nonce, and code hash for the contract address
3. **Get contract bytecode** - Fetches the actual contract bytecode if the account is a contract
4. **Iterate through all storage slots** - Uses database cursors to efficiently retrieve all storage key-value pairs

## Prerequisites

- A reth database with some data (you can sync a node or use a pre-synced database)
- Set the `RETH_DATADIR` environment variable to point to your reth data directory
- Set the `CONTRACT_ADDRESS` environment variable to provide target contract address

## Usage

```bash
# Set your reth data directory
export RETH_DATADIR="/path/to/your/reth/datadir"
# Set target contract address
export CONTRACT_ADDRESS="0x0..."

# Run the example
cargo run --example full-contract-state
```

## Code Structure

The example consists of:

- **`ContractState` struct** - Holds all contract state information
- **`extract_contract_state` function** - Main function that extracts contract state
- **`main` function** - Sets up the provider and demonstrates usage

## Key Concepts

### Provider Pattern
The example uses reth's provider pattern:
- `ProviderFactory` - Creates database connections
- `DatabaseProvider` - Provides low-level database access
- `StateProvider` - Provides high-level state access

### Database Cursors
For efficient storage iteration, the example uses database cursors:
- `cursor_dup_read` - Creates a cursor for duplicate key tables
- `seek_exact` - Positions cursor at specific key
- `next_dup` - Iterates through duplicate entries

## Output

The example will print:
- Contract address
- Account balance
- Account nonce
- Code hash
- Number of storage slots
- All storage key-value pairs

## Error Handling

The example includes proper error handling:
- Returns `None` if the contract doesn't exist
- Uses `ProviderResult` for database operation errors
- Gracefully handles missing bytecode or storage
