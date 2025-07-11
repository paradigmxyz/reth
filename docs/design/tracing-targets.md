# Tracing Targets

Reth uses the [`tracing`](https://docs.rs/tracing) crate for structured logging and observability. This document provides a comprehensive guide to all available tracing targets in the Reth codebase, helping developers understand what trace targets to enable when debugging different components or protocols.

## Table of Contents

- [Overview](#overview)
- [Tracing Target Naming Conventions](#tracing-target-naming-conventions)
- [Complete Target Reference](#complete-target-reference)
- [Log Level Guidelines](#log-level-guidelines)
- [Configuration Examples](#configuration-examples)
- [Debugging Common Scenarios](#debugging-common-scenarios)

## Overview

Reth's tracing system follows a hierarchical naming convention that groups related functionality under common prefixes. This makes it easier to enable or disable logging for specific subsystems while debugging.

### Metrics vs Traces

As mentioned in the [metrics documentation](metrics.md), **traces** are request-centric and help identify the path of operations through various services, while **metrics** are system-centric numeric measurements. Use traces for:

- **Contributors**: Profiling and understanding code flow
- **End-users**: Debugging complex infrastructure setups, especially RPC components
- **Development**: Understanding the lifecycle of operations like block processing, network events, or sync stages

## Tracing Target Naming Conventions

Reth uses a hierarchical naming pattern for tracing targets:

1. **Top-level component** (e.g., `reth`, `engine`, `net`, `sync`)
2. **Sub-component** (e.g., `reth::cli`, `engine::tree`, `net::tx`)
3. **Specific functionality** (e.g., `net::tx::propagation`, `sync::stages::headers`)

### Crates with Non-Standard Target Names

Several crates use target names that differ from their crate names for better readability or functional grouping:

| Crate Pattern | Target Pattern | Example |
|---------------|----------------|---------|
| `reth-cli-*` | `reth::cli` | CLI operations across all commands |
| `reth-transaction-pool` | `txpool` | Transaction pool operations |
| `reth-net-discv4` | `discv4` | Discovery v4 protocol |
| `reth-payload-*` | `payload_builder` | Block payload construction |
| `reth-engine-tree` | `engine::tree` | Engine API tree operations |

## Complete Target Reference

### Core Node Operations

#### CLI and Node Management
- **`reth::cli`** (227 occurrences) - Node startup, shutdown, and CLI command execution
  - Use for: Debugging node initialization, configuration issues, graceful shutdown
  - Example: `RUST_LOG=reth::cli=debug`

### Networking and P2P

#### Discovery Protocols
- **`discv4`** (45 occurrences) - Discovery v4 protocol operations
- **`net::discv5`** (17 occurrences) - Discovery v5 protocol operations
- **`disc::dns`** (8 occurrences) - DNS-based discovery

#### Network Operations
- **`net`** (26 occurrences) - General networking operations
- **`net::session`** (20 occurrences) - Peer session management and lifecycle
- **`net::peers`** (14 occurrences) - Peer connection management
- **`net::tx`** (42 occurrences) - Transaction propagation over network
- **`net::tx::propagation`** (4 occurrences) - Detailed transaction propagation mechanics
- **`net::nat`** (2 occurrences) - NAT traversal operations

#### Usage Example:
```bash
# Debug network connectivity issues
RUST_LOG=net=debug,net::session=trace

# Focus on transaction propagation
RUST_LOG=net::tx=debug,net::tx::propagation=trace
```

### Engine API and Consensus

#### Engine Tree Operations
- **`engine::tree`** (112 occurrences) - Core engine API block processing and validation
- **`engine::root`** (28 occurrences) - Engine root state management
- **`engine::root::sparse`** (12 occurrences) - Sparse engine root operations
- **`engine::persistence`** (5 occurrences) - Engine data persistence
- **`engine::stream::reorg`** (7 occurrences) - Chain reorganization handling
- **`engine::invalid_block_hooks::witness`** (6 occurrences) - Invalid block witness generation

#### Consensus Layer
- **`consensus::engine`** (6 occurrences) - Consensus engine operations
- **`consensus::debug-client`** (4 occurrences) - Consensus client debugging
- **`consensus::engine::sync`** (2 occurrences) - Consensus synchronization

#### Usage Example:
```bash
# Debug block processing issues
RUST_LOG=engine::tree=debug,engine::root=trace

# Investigate chain reorgs
RUST_LOG=engine::stream::reorg=trace,consensus::engine=debug
```

### RPC and API Layer

#### Ethereum RPC
- **`rpc::eth`** (65 occurrences) - Core Ethereum RPC methods (eth_*)
- **`rpc::eth::filter`** (2 occurrences) - Ethereum filter methods (eth_newFilter, etc.)
- **`rpc::eth::call`** (1 occurrence) - Contract call methods (eth_call, eth_estimateGas)
- **`rpc::eth::estimate`** (2 occurrences) - Gas estimation methods

#### Engine RPC
- **`rpc::engine`** (30 occurrences) - Engine API RPC methods (engine_*)
- **`rpc::flashbots`** (2 occurrences) - Flashbots MEV-related RPC methods

#### Other RPC Components
- **`rpc`** (4 occurrences) - General RPC operations
- **`rpc::historical`** (3 occurrences) - Historical data access methods
- **`rpc::sequencer`** (1 occurrence) - Sequencer-specific RPC methods
- **`rpc::fee`** (1 occurrence) - Fee-related RPC methods

#### Usage Example:
```bash
# Debug RPC performance issues
RUST_LOG=rpc::eth=debug,rpc::engine=debug

# Investigate gas estimation problems
RUST_LOG=rpc::eth::estimate=trace,rpc::eth::call=debug
```

### Synchronization Pipeline

#### Sync Coordination
- **`sync::pipeline`** (15 occurrences) - Overall sync pipeline coordination
- **`sync::metrics`** (1 occurrence) - Synchronization performance metrics

#### Sync Stages
- **`sync::stages::headers`** (8 occurrences) - Header downloading and validation
- **`sync::stages::execution`** (8 occurrences) - Block execution stage
- **`sync::stages::sender_recovery`** (5 occurrences) - Transaction sender recovery
- **`sync::stages::merkle::exec`** (5 occurrences) - Merkle tree computation
- **`sync::stages::transaction_lookup`** (4 occurrences) - Transaction index building
- **`sync::stages::bodies`** (2 occurrences) - Block body downloading

#### Usage Example:
```bash
# Debug sync performance
RUST_LOG=sync::pipeline=debug,sync::stages::headers=trace

# Investigate execution issues
RUST_LOG=sync::stages::execution=debug,sync::stages::merkle::exec=trace
```

### Data Providers and Storage

#### Provider Layer
- **`providers::blockchain`** (11 occurrences) - Blockchain data access
- **`providers::db`** (10 occurrences) - Database provider operations
- **`provider::static_file`** (10 occurrences) - Static file operations
- **`providers::static_file`** (2 occurrences) - Static file provider interface
- **`provider::storage_writer`** (5 occurrences) - Storage writing operations
- **`provider::historical_sp`** (2 occurrences) - Historical state providers

#### Storage Systems
- **`static_file`** (5 occurrences) - Static file management
- **`libmdbx`** (7 occurrences) - MDBX database operations
- **`storage::db::mdbx`** (4 occurrences) - MDBX storage implementation
- **`nippy-jar`** (5 occurrences) - Nippy-jar compression format

#### Usage Example:
```bash
# Debug storage issues
RUST_LOG=providers::db=debug,libmdbx=trace

# Investigate static file operations
RUST_LOG=provider::static_file=debug,static_file=trace
```

### Downloaders

#### Download Coordination
- **`downloaders`** (9 occurrences) - General downloader coordination
- **`downloaders::headers`** (20 occurrences) - Header downloading from peers
- **`downloaders::bodies`** (12 occurrences) - Block body downloading
- **`downloaders::file`** (15 occurrences) - File-based downloading operations

#### Usage Example:
```bash
# Debug download performance
RUST_LOG=downloaders::headers=debug,downloaders::bodies=debug

# Investigate download failures
RUST_LOG=downloaders=trace
```

### Transaction Pool

- **`txpool`** (32 occurrences) - Transaction pool operations including:
  - Transaction validation and inclusion
  - Pool management and eviction
  - Gas price ordering
  - Mempool state management

#### Usage Example:
```bash
# Debug transaction pool issues
RUST_LOG=txpool=debug

# Investigate transaction propagation
RUST_LOG=txpool=debug,net::tx=debug
```

### Payload Building

- **`payload_builder`** (35 occurrences) - Block payload construction including:
  - Transaction selection and ordering
  - Gas limit management
  - MEV integration
  - Payload optimization

#### Usage Example:
```bash
# Debug block building
RUST_LOG=payload_builder=debug

# Investigate payload building performance
RUST_LOG=payload_builder=trace,txpool=debug
```

### Trie Operations

#### Core Trie Operations
- **`trie::sparse`** (21 occurrences) - Sparse trie operations and optimizations
- **`trie::parallel_sparse`** (11 occurrences) - Parallel sparse trie processing
- **`trie::node_iter`** (7 occurrences) - Trie node iteration
- **`trie::walker`** (5 occurrences) - Trie walking and traversal

#### Proof Generation
- **`trie::proof_task`** (9 occurrences) - Trie proof generation tasks
- **`trie::proof::blinded`** (4 occurrences) - Blinded trie proof operations
- **`trie::parallel_proof`** (4 occurrences) - Parallel proof generation

#### State Root Computation
- **`trie::storage_root`** (3 occurrences) - Storage trie root calculations
- **`trie::parallel_state_root`** (3 occurrences) - Parallel state root computation
- **`trie::state_root`** (2 occurrences) - State root calculations
- **`trie::loader`** (3 occurrences) - Trie data loading operations

#### Usage Example:
```bash
# Debug trie computation issues
RUST_LOG=trie::sparse=debug,trie::proof_task=trace

# Investigate state root problems
RUST_LOG=trie::state_root=debug,trie::storage_root=debug
```

### Database Pruning

- **`pruner`** (28 occurrences) - Database pruning operations including:
  - Data cleanup and archival
  - Storage optimization
  - Historical data management
- **`pruner::test`** (1 occurrence) - Pruning test utilities

#### Usage Example:
```bash
# Debug pruning operations
RUST_LOG=pruner=debug

# Investigate pruning performance
RUST_LOG=pruner=trace
```

### Execution Extensions (ExEx)

#### ExEx Management
- **`exex::manager`** (11 occurrences) - ExEx lifecycle management
- **`exex::notifications`** (8 occurrences) - Event notification system
- **`exex::backfill`** (8 occurrences) - Historical data backfilling
- **`exex::wal`** (4 occurrences) - Write-ahead log operations
- **`exex::wal::storage`** (4 occurrences) - WAL storage management

#### Usage Example:
```bash
# Debug ExEx issues
RUST_LOG=exex::manager=debug,exex::notifications=trace

# Investigate backfill operations
RUST_LOG=exex::backfill=debug,exex::wal=trace
```

### Specialized Components

#### Era Import/Export
- **`era::history::export`** (5 occurrences) - Era history export operations
- **`era::history::import`** (2 occurrences) - Era history import operations

#### RESS Protocol
- **`ress::net::connection`** (13 occurrences) - RESS network connections
- **`ress::net`** (2 occurrences) - RESS networking
- **`reth::ress_provider`** (10 occurrences) - RESS provider operations
- **`reth::ress`** (1 occurrence) - General RESS operations

#### Alloy Integration
- **`alloy-provider`** (4 occurrences) - Alloy provider integration

#### Usage Example:
```bash
# Debug era operations
RUST_LOG=era::history::export=debug,era::history::import=debug

# Investigate RESS protocol
RUST_LOG=ress::net=debug,reth::ress_provider=trace
```

## Log Level Guidelines

### Error Level (`error!`)
Use for **fatal errors** that require immediate attention:
- Node shutdown due to critical failures
- Consensus violations
- Irrecoverable chain state issues
- Critical infrastructure failures

```rust
error!(target: "reth::cli", %error, "Node shutting down due to critical error");
```

### Warn Level (`warn!`)
Use for **recoverable issues** that may impact performance:
- Network timeouts and retry attempts
- State inconsistencies that can be resolved
- Protocol violations from peers
- Resource constraints

```rust
warn!(target: "net::session", ?peer_id, "Connection timeout, attempting retry");
```

### Info Level (`info!`)
Use for **important operational events**:
- Node startup and shutdown
- Major state transitions
- Successful completion of significant operations
- Configuration changes

```rust
info!(target: "sync::pipeline", block_number = %block.number, "Sync pipeline completed");
```

### Debug Level (`debug!`)
Use for **detailed operational information**:
- Transaction processing details
- Block building progress
- Network events and peer interactions
- State transitions and validations

```rust
debug!(target: "payload_builder", 
       payload_id = %id, 
       tx_count = txs.len(), 
       "Building payload with transactions");
```

### Trace Level (`trace!`)
Use for **fine-grained tracking** and performance analysis:
- Individual packet sending/receiving
- Algorithm internals and step-by-step operations
- Performance timing and budget tracking
- Detailed state changes

```rust
trace!(target: "net::tx", 
       ?transaction_hash, 
       ?peer_id, 
       elapsed = ?start.elapsed(),
       "Propagated transaction to peer");
```

## Configuration Examples

### Basic Configuration

Set tracing levels using the `RUST_LOG` environment variable:

```bash
# Enable debug logging for all components
RUST_LOG=debug

# Enable specific components
RUST_LOG=engine::tree=debug,rpc::eth=info

# Mixed levels for different components
RUST_LOG=reth::cli=info,net=debug,txpool=trace
```

### Advanced Filtering

```bash
# Enable all engine operations with detailed trie information
RUST_LOG=engine=debug,trie=trace

# Focus on networking and transaction propagation
RUST_LOG=net=debug,net::tx=trace,txpool=debug

# Debug RPC performance with engine context
RUST_LOG=rpc=debug,engine::tree=info,payload_builder=debug

# Comprehensive sync debugging
RUST_LOG=sync=debug,downloaders=debug,providers=info
```

### JSON Output for Production

```bash
# Enable JSON formatted logs for production monitoring
RUST_LOG=info reth node --log.format json

# Enable specific components with JSON output
RUST_LOG=engine=debug,rpc=info reth node --log.format json
```

## Debugging Common Scenarios

### Sync Issues
```bash
# Comprehensive sync debugging
RUST_LOG=sync::pipeline=debug,sync::stages::headers=trace,sync::stages::execution=debug,downloaders=debug

# Focus on specific sync stage
RUST_LOG=sync::stages::headers=trace,downloaders::headers=debug,net=info
```

### RPC Performance Issues
```bash
# Debug RPC response times
RUST_LOG=rpc::eth=debug,engine::tree=debug,providers=debug

# Investigate specific RPC methods
RUST_LOG=rpc::eth::call=trace,rpc::eth::estimate=debug,txpool=debug
```

### Network Connectivity Problems
```bash
# Debug peer connections and discovery
RUST_LOG=net=debug,net::session=trace,discv4=debug,net::peers=debug

# Focus on transaction propagation
RUST_LOG=net::tx=trace,net::tx::propagation=debug,txpool=debug
```

### Block Building Issues
```bash
# Debug payload construction
RUST_LOG=payload_builder=trace,txpool=debug,engine::tree=debug

# Investigate gas estimation problems
RUST_LOG=rpc::eth::estimate=trace,rpc::eth::call=debug,payload_builder=debug
```

### Storage and Database Issues
```bash
# Debug storage operations
RUST_LOG=providers::db=debug,libmdbx=trace,static_file=debug

# Investigate pruning operations
RUST_LOG=pruner=debug,providers=debug,storage::db::mdbx=trace
```

### Trie Computation Problems
```bash
# Debug state root computation
RUST_LOG=trie::state_root=trace,trie::sparse=debug,trie::proof_task=debug

# Investigate parallel trie operations
RUST_LOG=trie::parallel_sparse=trace,trie::parallel_proof=debug
```

## Best Practices

1. **Start with higher log levels** (info/debug) and increase verbosity (trace) for specific components as needed
2. **Use hierarchical targeting** - start with broad targets (`net`) and narrow down (`net::tx::propagation`)
3. **Combine related targets** - when debugging sync, include both `sync` and `downloaders` targets
4. **Monitor performance impact** - trace-level logging can significantly impact performance
5. **Use structured fields** - leverage the structured nature of tracing for better filtering and analysis
6. **Consider output format** - use JSON format for production environments and log aggregation

## Integration with Observability Tools

### Prometheus/Grafana
While this document covers tracing, remember that [metrics](metrics.md) are better suited for monitoring system health and performance over time.

### OpenTelemetry
Reth's tracing can be integrated with OpenTelemetry for distributed tracing across multiple services.

### Log Aggregation
Use JSON format output with tools like ELK stack, Loki, or similar log aggregation systems for production deployments.

## Contributing

When adding new tracing to the codebase:

1. **Follow the naming convention** - use hierarchical targets that match the component structure
2. **Choose appropriate log levels** - follow the guidelines above
3. **Use structured fields** - include relevant context like `block_number`, `peer_id`, `transaction_hash`
4. **Update this documentation** - add new targets to the appropriate sections
5. **Test performance impact** - ensure trace-level logging doesn't significantly impact performance

For questions about tracing targets or to suggest improvements to this documentation, please open an issue or discussion in the [Reth repository](https://github.com/paradigmxyz/reth).