# Reth Architecture: Entry Point to Component Flow

## 🚀 Entry Point & Initialization Flow

```mermaid
graph TB
    subgraph "Entry Point"
        Main["bin/reth/main.rs<br/>• Parse CLI args<br/>• Install signal handlers<br/>• Set up allocator"]
    end
    
    Main --> CLI["Cli::parse()"]
    CLI --> NodeBuilder["NodeBuilder<br/>• Configure node types<br/>• Set up components"]
    
    subgraph "Node Builder Pipeline"
        NodeBuilder --> ConfigTypes["1. Configure Types<br/>• NodeTypes<br/>• Primitives<br/>• Engine types"]
        ConfigTypes --> InitDB["2. Initialize Database<br/>• MDBX setup<br/>• Static files<br/>• BlockchainProvider"]
        InitDB --> BuildComponents["3. Build Components<br/>• Via NodeComponentsBuilder"]
    end
    
    subgraph "Core Components Assembly"
        BuildComponents --> ExecutorBuilder["ExecutorBuilder<br/>• EVM config<br/>• State execution<br/>• Block processing"]
        BuildComponents --> PoolBuilder["PoolBuilder<br/>• Transaction pool<br/>• Mempool management<br/>• Transaction validation"]
        BuildComponents --> NetworkBuilder["NetworkBuilder<br/>• P2P networking<br/>• Discovery (discv4/discv5)<br/>• Sync protocols"]
        BuildComponents --> PayloadBuilder["PayloadBuilder<br/>• Block production<br/>• MEV integration<br/>• Payload assembly"]
    end
    
    ExecutorBuilder --> Launch
    PoolBuilder --> Launch
    NetworkBuilder --> Launch
    PayloadBuilder --> Launch
    
    subgraph "Launch & Runtime"
        Launch["Launch Node<br/>• Start all services<br/>• Initialize RPC<br/>• Begin syncing"]
        Launch --> NodeHandle["NodeHandle<br/>• Full access to components<br/>• Runtime control<br/>• Monitoring"]
    end
    
    style Main fill:#f9f,stroke:#333,stroke-width:4px
    style NodeBuilder fill:#bbf,stroke:#333,stroke-width:2px
    style Launch fill:#bfb,stroke:#333,stroke-width:2px
```

## 📊 Component Dependency Graph

```mermaid
graph LR
    subgraph "Storage Layer"
        DB["Database (MDBX)<br/>• Block storage<br/>• State storage<br/>• Receipts & logs"]
        StaticFiles["Static Files<br/>• Headers<br/>• Bodies<br/>• Transactions"]
        DB -.-> StaticFiles
    end
    
    subgraph "Provider Layer"
        BlockchainProvider["BlockchainProvider<br/>• Unified data access<br/>• Chain queries<br/>• State access"]
        DB --> BlockchainProvider
        StaticFiles --> BlockchainProvider
    end
    
    subgraph "Execution Layer"
        EVM["EVM<br/>• revm integration<br/>• State transitions<br/>• Gas accounting"]
        Executor["Executor<br/>• Block execution<br/>• Transaction processing<br/>• State updates"]
        BlockchainProvider --> EVM
        EVM --> Executor
    end
    
    subgraph "Consensus Layer"
        Consensus["Consensus<br/>• Block validation<br/>• Fork choice<br/>• Chain rules"]
        EngineAPI["Engine API<br/>• CL communication<br/>• Payload handling<br/>• Fork choice updates"]
        BlockchainProvider --> Consensus
        Executor --> Consensus
        Consensus --> EngineAPI
    end
    
    subgraph "Network Layer"
        P2P["P2P Network<br/>• Peer management<br/>• Message routing<br/>• Protocol handling"]
        Discovery["Discovery<br/>• discv4/discv5<br/>• Peer discovery<br/>• NAT traversal"]
        Sync["Sync<br/>• Staged sync<br/>• State download<br/>• Block propagation"]
        P2P --> Discovery
        P2P --> Sync
        BlockchainProvider --> Sync
    end
    
    subgraph "Transaction Pool"
        TxPool["Transaction Pool<br/>• Pending txs<br/>• Validation<br/>• Ordering"]
        TxPropagation["Tx Propagation<br/>• Broadcast<br/>• Gossip<br/>• Filtering"]
        BlockchainProvider --> TxPool
        P2P --> TxPropagation
        TxPool --> TxPropagation
    end
    
    subgraph "RPC Layer"
        JSONRPC["JSON-RPC<br/>• eth_ namespace<br/>• debug_ namespace<br/>• Custom methods"]
        WebSocket["WebSocket<br/>• Subscriptions<br/>• Real-time updates"]
        BlockchainProvider --> JSONRPC
        TxPool --> JSONRPC
        JSONRPC --> WebSocket
    end
    
    subgraph "Block Production"
        PayloadBuilderService["Payload Builder<br/>• Block assembly<br/>• Transaction selection<br/>• MEV integration"]
        TxPool --> PayloadBuilderService
        Executor --> PayloadBuilderService
        PayloadBuilderService --> EngineAPI
    end
    
    style DB fill:#ffd,stroke:#333,stroke-width:2px
    style BlockchainProvider fill:#dff,stroke:#333,stroke-width:3px
    style EngineAPI fill:#fdf,stroke:#333,stroke-width:2px
```

## 🔄 Data Flow Through Components

```mermaid
sequenceDiagram
    participant CLI as CLI/Main
    participant Builder as NodeBuilder
    participant DB as Database
    participant Provider as BlockchainProvider
    participant Network as P2P Network
    participant TxPool as Transaction Pool
    participant Executor as Executor/EVM
    participant Engine as Engine API
    participant RPC as RPC Server
    
    CLI->>Builder: Parse config & initialize
    Builder->>DB: Initialize MDBX + Static Files
    Builder->>Provider: Create BlockchainProvider
    Builder->>Network: Start P2P networking
    Builder->>TxPool: Initialize transaction pool
    Builder->>Executor: Configure EVM & executor
    Builder->>Engine: Setup Engine API
    Builder->>RPC: Start RPC server
    
    Note over Network: Begin peer discovery
    Network->>Network: Connect to peers
    
    loop Sync Process
        Network->>Provider: Download blocks
        Provider->>DB: Store blocks
        Network->>Executor: Execute blocks
        Executor->>DB: Update state
    end
    
    loop Transaction Flow
        RPC->>TxPool: New transaction
        TxPool->>Network: Propagate tx
        Network->>TxPool: Receive remote tx
    end
    
    loop Block Production
        Engine->>TxPool: Get pending txs
        TxPool->>Executor: Validate & execute
        Executor->>Engine: Return payload
        Engine->>Network: Propagate block
    end
```

## 🏗️ Stage Pipeline Architecture

```mermaid
graph TD
    subgraph "Staged Sync Pipeline"
        Headers["Headers Stage<br/>• Download headers<br/>• Validate chain<br/>• Build skeleton"]
        Bodies["Bodies Stage<br/>• Download block bodies<br/>• Transaction data<br/>• Uncle blocks"]
        Senders["Senders Recovery<br/>• Recover tx senders<br/>• ECDSA recovery<br/>• Parallel processing"]
        Execution["Execution Stage<br/>• Execute transactions<br/>• Update state<br/>• Generate receipts"]
        MerkleUnwind["Merkle Unwind<br/>• Unwind state changes<br/>• Reorg handling"]
        AccountHashing["Account Hashing<br/>• Hash addresses<br/>• Prepare for trie"]
        StorageHashing["Storage Hashing<br/>• Hash storage keys<br/>• Optimize lookups"]
        MerkleExecute["Merkle Execute<br/>• Build state trie<br/>• Calculate roots"]
        TxLookup["Transaction Lookup<br/>• Build tx indices<br/>• Enable fast queries"]
        IndexHistory["Index History<br/>• Account history<br/>• Storage history"]
        
        Headers --> Bodies
        Bodies --> Senders
        Senders --> Execution
        Execution --> MerkleUnwind
        MerkleUnwind --> AccountHashing
        AccountHashing --> StorageHashing
        StorageHashing --> MerkleExecute
        MerkleExecute --> TxLookup
        TxLookup --> IndexHistory
    end
    
    style Headers fill:#e1f5fe,stroke:#01579b,stroke-width:2px
    style Execution fill:#fff3e0,stroke:#e65100,stroke-width:2px
    style MerkleExecute fill:#f3e5f5,stroke:#4a148c,stroke-width:2px
```

## 🎯 Key Component Interactions

### 1. **Main Entry → NodeBuilder**
- `bin/reth/main.rs` is the entry point
- Parses CLI arguments via `Cli::parse()`
- Creates `NodeBuilder` with configuration
- Launches node with specified chain (e.g., `EthereumNode`)

### 2. **NodeBuilder → Component Assembly**
The NodeBuilder follows a strict initialization order:
1. **Database Setup**: Initializes MDBX and static files
2. **Provider Creation**: Creates `BlockchainProvider` for unified data access
3. **Component Building**: Uses `NodeComponentsBuilder` to create:
   - **Executor**: EVM and state execution
   - **Transaction Pool**: Mempool management
   - **Network**: P2P and sync protocols
   - **Payload Builder**: Block production

### 3. **Runtime Component Connections**

#### Storage → Everything
- All components read from `BlockchainProvider`
- Provider abstracts MDBX + static files
- Provides chain data, state, receipts

#### Network → Sync → Executor
- Network downloads blocks from peers
- Sync stages process blocks sequentially
- Executor updates state via EVM

#### Transaction Pool → Network & RPC
- Receives transactions from RPC and P2P
- Validates and orders transactions
- Propagates to peers via network

#### Engine API → Consensus Layer
- Receives payloads from consensus client
- Triggers block production
- Updates fork choice

#### Executor → State
- Processes transactions through revm
- Updates account states
- Generates receipts and logs

### 4. **Service Lifecycle**

```
Start: CLI → NodeBuilder → Database → Components → Services
Run:   Network Sync ← → Transaction Pool ← → Block Production
Stop:  Graceful shutdown of all services in reverse order
```

## 🔧 Extension Points

The architecture provides several extension points:

1. **ExEx (Execution Extensions)**: Hook into block execution
2. **RPC Modules**: Add custom RPC methods
3. **Network Protocols**: Add custom P2P protocols
4. **Payload Attributes**: Customize block building
5. **Component Builders**: Replace default components

## 📝 Summary

Reth's architecture follows a clear flow from entry point to fully operational node:

1. **Entry** (`main.rs`) → **Configuration** (CLI parsing)
2. **Builder** (NodeBuilder) → **Database** (MDBX setup)
3. **Components** (via builders) → **Services** (launch)
4. **Runtime** (NodeHandle) → **Operation** (sync, validate, produce blocks)

Each component has clear dependencies and interfaces, making the system modular and extensible while maintaining type safety throughout.