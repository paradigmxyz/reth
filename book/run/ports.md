# Ports

This section provides essential information about the ports used by the system, their primary purposes, and recommendations for exposure settings.

## Peering Ports

- **Port:** 30303
- **Protocol:** TCP and UDP
- **Purpose:** Peering with other nodes for synchronization of blockchain data. Nodes communicate through this port to maintain network consensus and share updated information.
- **Exposure Recommendation:** This port should be exposed to enable seamless interaction and synchronization with other nodes in the network.

## Metrics Port

- **Port:** 9001
- **Protocol:** TCP
- **Purpose:** This port is designated for serving metrics related to the system's performance and operation. It allows internal monitoring and data collection for analysis.
- **Exposure Recommendation:** By default, this port should not be exposed to the public. It is intended for internal monitoring and analysis purposes.

## HTTP RPC Port

- **Port:** 8545
- **Protocol:** TCP
- **Purpose:** Port 8545 provides an HTTP-based Remote Procedure Call (RPC) interface. It enables external applications to interact with the blockchain by sending requests over HTTP.
- **Exposure Recommendation:** Similar to the metrics port, exposing this port to the public is not recommended by default due to security considerations.

## WS RPC Port

- **Port:** 8546
- **Protocol:** TCP
- **Purpose:** Port 8546 offers a WebSocket-based Remote Procedure Call (RPC) interface. It allows real-time communication between external applications and the blockchain.
- **Exposure Recommendation:** As with the HTTP RPC port, the WS RPC port should not be exposed by default for security reasons.

## Engine API Port

- **Port:** 8551
- **Protocol:** TCP
- **Purpose:** Port 8551 facilitates communication between specific components, such as "reth" and "CL" (assuming their definitions are understood within the context of the system). It enables essential internal processes.
- **Exposure Recommendation:** This port is not meant to be exposed to the public by default. It should be reserved for internal communication between vital components of the system.
