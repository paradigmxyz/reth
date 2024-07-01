#![allow(dead_code)]

use discv5::{enr::secp256k1::rand, Enr, Event, ListenConfig};
use reth::network::config::SecretKey;
use reth_discv5::{enr::EnrCombinedKeyWrapper, Config, Discv5};
use reth_network_peers::NodeRecord;
use reth_tracing::tracing::info;
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc;

pub(crate) mod cli_ext;

/// Helper struct to manage a discovery node using discv5.
pub(crate) struct DiscV5ExEx {
    /// The inner discv5 instance.
    inner: Discv5,
    /// The node record of the discv5 instance.
    node_record: NodeRecord,
    /// The events stream of the discv5 instance.
    events: mpsc::Receiver<discv5::Event>,
}

impl DiscV5ExEx {
    /// Starts a new discv5 node.
    pub async fn new(udp_port: u16, tcp_port: u16) -> eyre::Result<DiscV5ExEx> {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port}").parse()?;
        let rlpx_addr: SocketAddr = format!("127.0.0.1:{tcp_port}").parse()?;

        let discv5_listen_config = ListenConfig::from(discv5_addr);
        let discv5_config = Config::builder(rlpx_addr)
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        let (discv5, events, node_record) = Discv5::start(&secret_key, discv5_config).await?;
        Ok(Self { inner: discv5, events, node_record })
    }

    /// Adds a node to the table if its not already present.
    pub fn add_node(&mut self, enr: Enr) -> eyre::Result<()> {
        let reth_enr: enr::Enr<SecretKey> = EnrCombinedKeyWrapper(enr.clone()).into();
        self.inner.add_node(reth_enr)?;
        Ok(())
    }

    /// Returns the local ENR of the discv5 node.
    pub fn local_enr(&self) -> Enr {
        self.inner.with_discv5(|discv5| discv5.local_enr())
    }
}

impl Future for DiscV5ExEx {
    type Output = eyre::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut();
        loop {
            match ready!(this.events.poll_recv(cx)) {
                Some(evt) => {
                    if let Event::SessionEstablished(enr, socket_addr) = evt {
                        info!(?enr, ?socket_addr, "Session established with a new peer.");
                    }
                }
                None => return Poll::Ready(Ok(())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::network::DiscV5ExEx;
    use tracing::info;

    #[tokio::test]
    async fn can_establish_discv5_session_with_peer() {
        reth_tracing::init_test_tracing();
        let mut node_1 = DiscV5ExEx::new(30301, 30303).await.unwrap();
        let node_1_enr = node_1.local_enr();

        let mut node_2 = DiscV5ExEx::new(30302, 30303).await.unwrap();

        let node_2_enr = node_2.local_enr();

        info!(?node_1_enr, ?node_2_enr, "Started discovery nodes.");

        // add node_2 to node_1 table
        node_1.add_node(node_2_enr.clone()).unwrap();

        // verify node_2 is in node_1 table
        assert!(node_1
            .inner
            .with_discv5(|discv5| discv5.table_entries_id().contains(&node_2_enr.node_id())));

        // send ping from node_1 to node_2
        node_1.inner.with_discv5(|discv5| discv5.send_ping(node_2_enr.clone())).await.unwrap();

        // verify they both established a session
        let event_2_v5 = node_2.events.recv().await.unwrap();
        let event_1_v5 = node_1.events.recv().await.unwrap();
        assert!(matches!(
            event_1_v5,
            discv5::Event::SessionEstablished(node, socket) if node == node_2_enr && socket == node_2_enr.udp4_socket().unwrap().into()
        ));
        assert!(matches!(
            event_2_v5,
            discv5::Event::SessionEstablished(node, socket) if node == node_1_enr && socket == node_1_enr.udp4_socket().unwrap().into()
        ));

        // verify node_1 is in
        let event_2_v5 = node_2.events.recv().await.unwrap();
        assert!(matches!(
            event_2_v5,
            discv5::Event::NodeInserted { node_id, replaced } if node_id == node_1_enr.node_id() && replaced.is_none()
        ));
    }
}
