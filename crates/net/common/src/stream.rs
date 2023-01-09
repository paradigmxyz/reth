use std::net::SocketAddr;
use tokio::net::TcpStream;
/// This trait is for instrumenting a TCPStream with a socket addr
pub trait HasRemoteAddr {
    /// Maybe returns a [`SocketAddr`]
    fn remote_addr(&self) -> Option<SocketAddr>;
}

impl HasRemoteAddr for TcpStream {
    fn remote_addr(&self) -> Option<SocketAddr> {
        self.peer_addr().ok()
    }
}
