#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Utilities for interacting with ethereum's peer to peer network.

pub mod anchor;

#[cfg(test)]
mod tests {
    use arrayvec::ArrayString;
    use async_trait::async_trait;
    use bytes::Bytes;
    use devp2p_rs::{
        rlpx::Swarm, CapabilityName, CapabilityServer, CapabilityVersion, Discovery, InboundEvent,
        ListenOptions, Message, OutboundEvent, PeerId, StaticNodes,
    };
    use secp256k1::{rand::thread_rng, SecretKey};
    use std::{
        collections::{BTreeMap, HashMap},
        net::SocketAddr,
        num::NonZeroUsize,
        str::FromStr,
        sync::{atomic::AtomicBool, Arc},
        time::Duration,
    };
    use tokio_stream::StreamMap;

    // this exists so we can create simple connections for testing that don't have a subprotocol
    // TODO: test passing a simple message with a channel
    pub(crate) struct EmptyCapabilityServer {}

    #[async_trait]
    impl CapabilityServer for EmptyCapabilityServer {
        async fn on_peer_event(&self, peer: PeerId, event: InboundEvent) {}
        fn on_peer_connect(&self, peer: PeerId, caps: HashMap<CapabilityName, CapabilityVersion>) {}
        async fn next(&self, peer: PeerId) -> OutboundEvent {
            OutboundEvent::Message {
                capability_name: CapabilityName(ArrayString::from_str("none").unwrap()),
                message: Message { id: 0, data: Bytes::new() },
            }
        }
    }

    #[tokio::test]
    async fn simple_connection_test() {
        let alice_key = SecretKey::new(&mut thread_rng());
        let alice_no_new_peers_handle = Arc::new(AtomicBool::new(true));
        let alice_capability_server = Arc::new(EmptyCapabilityServer {});
        let alice_listen_port = 30303;
        let alice_listen_addr = format!("0.0.0.0:{}", alice_listen_port);

        let bob_key = SecretKey::new(&mut thread_rng());
        let bob_no_new_peers_handle = Arc::new(AtomicBool::new(true));
        let bob_capability_server = Arc::new(EmptyCapabilityServer {});
        let bob_listen_port = 30304;
        let bob_listen_addr = format!("0.0.0.0:{}", bob_listen_port);

        let alice = Swarm::builder()
            .with_listen_options(ListenOptions::new(
                StreamMap::new(),
                0,
                NonZeroUsize::new(1).unwrap(),
                alice_listen_addr.parse().unwrap(),
                None,
                alice_no_new_peers_handle,
            ))
            .with_client_version(format!("reth/v{}", env!("CARGO_PKG_VERSION")))
            .build(BTreeMap::new(), alice_capability_server, alice_key);

        // devp2p-rs requires the user specify a H512 PeerId per node
        let mut bob_static_nodes = HashMap::new();
        let alice_socket_addr: SocketAddr = alice_listen_addr.parse().unwrap();
        bob_static_nodes.insert(alice_socket_addr, PeerId::zero());

        // there are no methods to "dial once" in devp2p-rs currently, you must create tasks and
        // instantiate the `Swarm` with those tasks.
        let mut discovery_tasks: StreamMap<String, Discovery> = StreamMap::new();
        let connect_to_alice = StaticNodes::new(bob_static_nodes, Duration::new(5, 0));
        discovery_tasks.insert("dial alice".to_string(), Box::pin(connect_to_alice));
        let bob = Swarm::builder()
            .with_listen_options(ListenOptions::new(
                discovery_tasks,
                0,
                NonZeroUsize::new(1).unwrap(),
                bob_listen_addr.parse().unwrap(),
                None,
                bob_no_new_peers_handle,
            ))
            .with_client_version(format!("reth/v{}", env!("CARGO_PKG_VERSION")))
            .build(BTreeMap::new(), bob_capability_server, bob_key);

        tokio::spawn(async move { alice.await });

        tokio::spawn(async move { bob.await });

        // is it possible to confirm the dial succeeded without sending a message?
        // TODO: send message from alice to bob
        todo!()
    }
}
