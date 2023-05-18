use enr::{k256::ecdsa::SigningKey, Enr, EnrPublicKey};
use reth_primitives::PeerId;
use std::{net::SocketAddr, time::Duration};

/// The timeout for tests that create a GethInstance
pub const GETH_TIMEOUT: Duration = Duration::from_secs(60);

/// Obtains a PeerId from an ENR. In this case, the PeerId represents the public key contained in
/// the ENR.
pub fn enr_to_peer_id(enr: Enr<SigningKey>) -> PeerId {
    // In the following tests, methods which accept a public key expect it to contain the public
    // key in its 64-byte encoded (uncompressed) form.
    enr.public_key().encode_uncompressed().into()
}

// copied from ethers-rs
/// A bit of hack to find an unused TCP port.
///
/// Does not guarantee that the given port is unused after the function exists, just that it was
/// unused before the function started (i.e., it does not reserve a port).
pub fn unused_port() -> u16 {
    unused_tcp_addr().port()
}

/// Finds an unused tcp address
pub fn unused_tcp_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("Failed to create TCP listener to find unused port");
    listener.local_addr().expect("Failed to read TCP listener local_addr to find unused port")
}

/// Finds an unused udp port
pub fn unused_udp_port() -> u16 {
    unused_udp_addr().port()
}
/// Finds an unused udp address
pub fn unused_udp_addr() -> SocketAddr {
    let udp_listener = std::net::UdpSocket::bind("127.0.0.1:0")
        .expect("Failed to create UDP listener to find unused port");
    udp_listener.local_addr().expect("Failed to read UDP listener local_addr to find unused port")
}

/// Finds a single port that is unused for both TCP and UDP.
pub fn unused_tcp_and_udp_port() -> u16 {
    loop {
        let port = unused_port();
        if std::net::UdpSocket::bind(format!("127.0.0.1:{port}")).is_ok() {
            return port
        }
    }
}

/// Creates two unused SocketAddrs, intended for use as the p2p (TCP) and discovery ports (UDP) for
/// new reth instances.
pub fn unused_tcp_udp() -> (SocketAddr, SocketAddr) {
    (unused_tcp_addr(), unused_udp_addr())
}
