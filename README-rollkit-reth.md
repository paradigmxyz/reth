# Rollkit-Reth Docker Setup

This setup provides a Docker container running `rollkit-reth` instead of the standard `op-reth`, while maintaining the same configuration and flags.

## Overview

This configuration is based on the original `op-reth` docker-compose setup but modified to:
- Build and run `rollkit-reth` binary from the reth codebase
- Use the same JWT authentication, volumes, and network configuration
- Maintain compatibility with existing genesis and configuration files

## Files

- `rollkit-reth-docker-compose.yml` - Docker Compose configuration for rollkit-reth
- `reth/Dockerfile.rollkit` - Custom Dockerfile to build rollkit-reth binary

## Prerequisites

1. Ensure you have the reth source code in the `./reth` directory
2. Have Docker and Docker Compose installed
3. Prepare your chain configuration files:
   - `./chain/genesis.json` - Genesis configuration
   - `./jwttoken/` - Directory for JWT tokens (created automatically)

## Usage

### 1. Start the rollkit-reth node

```bash
docker-compose -f rollkit-reth-docker-compose.yml up -d
```

### 2. View logs

```bash
# View all logs
docker-compose -f rollkit-reth-docker-compose.yml logs

# View rollkit-reth logs only
docker-compose -f rollkit-reth-docker-compose.yml logs rollkit-reth

# Follow logs in real-time
docker-compose -f rollkit-reth-docker-compose.yml logs -f rollkit-reth
```

### 3. Stop the services

```bash
docker-compose -f rollkit-reth-docker-compose.yml down
```

### 4. Rebuild after code changes

```bash
# Rebuild the rollkit-reth image
docker-compose -f rollkit-reth-docker-compose.yml build rollkit-reth

# Or rebuild and restart
docker-compose -f rollkit-reth-docker-compose.yml up --build -d
```

## Key Differences from OP-Reth Setup

1. **Binary**: Uses `rollkit-reth` instead of `op-reth`
2. **Image**: Builds from local Dockerfile instead of pulling from registry
3. **Features**: Includes rollkit-specific Engine API enhancements
4. **Container Name**: Changed from `op-reth` to `rollkit-reth`

## Configuration

The rollkit-reth node uses the same configuration flags as the original setup:

- **Metrics**: Available on port 9001
- **HTTP RPC**: Available on port 8545
- **WebSocket RPC**: Available on port 8546
- **Engine API**: Available on port 8551 (authenticated)
- **P2P**: Uses port 30303

## Volumes

- `./chain:/root/chain:ro` - Genesis and chain configuration (read-only)
- `./jwttoken:/root/jwt:ro` - JWT authentication tokens (read-only)
- `logs:/root/logs` - Log files

## Rollkit Features

The rollkit-reth binary includes:
- Custom Engine API support for rollkit integration
- Enhanced payload building for rollup transactions
- Rollkit-specific consensus mechanisms
- Compatible with standard Ethereum JSON-RPC APIs

## Troubleshooting

### Build Issues
- Ensure the `./reth` directory contains the complete reth source code
- Check that the `rollkit-payload-builder` crate is present
- Verify Docker has sufficient resources for the Rust compilation

### Runtime Issues
- Check that the genesis file is properly formatted
- Verify JWT tokens are generated correctly
- Ensure ports are not conflicting with other services

### Network Issues
- Confirm firewall settings allow the required ports
- Check that the genesis configuration matches your network requirements

## Development

For development purposes, you can:

1. **Use debug build** (faster compilation):
   ```bash
   docker-compose -f rollkit-reth-docker-compose.yml build --build-arg BUILD_PROFILE=dev rollkit-reth
   ```

2. **Enable verbose logging**:
   Add `--verbosity=4` to the command section in the docker-compose file

3. **Mount source code** for development:
   Add a volume mount for live code updates (requires rebuilding the binary) 