//! Wrapper around [`discv5::Discv5`] that supports downgrade to [`Discv4`].

use std::{
    collections::HashSet,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

use derive_more::From;
use discv5::{enr::EnrPublicKey, IpMode};
use futures::{
    stream::{select, Select},
    Stream, StreamExt,
};
use reth_discv4::{
    error::Discv4Error,
    secp256k1::{self, PublicKey, SecretKey},
    DiscoveryUpdate, Discv4, Discv4Config,
};
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    NodeRecord, PeerId,
};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::{
    filter::{FilterDiscovered, FilterOutcome, MustIncludeChain},
    DiscV5, DiscV5Config, HandleDiscv5,
};

/// Errors interfacing between [`discv5::Discv5`] and [`Discv4`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failure sending notification to [`Discv4`].
    #[error("failed notifying discv4 of update to discv5 kbuckets")]
    NotifyKBucketsUpdateFailed(watch::error::SendError<()>),
    /// Error interfacing with [`discv5::Discv5`].
    #[error(transparent)]
    DiscV5Error(#[from] super::Error),
    /// Failed to initialize [`Discv4`].
    #[error("init failed, {0}")]
    Discv4InitFailure(io::Error),
    /// Error interfacing with [`Discv4`].
    #[error(transparent)]
    Discv4Error(#[from] Discv4Error),
}

/// Wraps [`discv5::Discv5`] supporting downgrade to [`Discv4`].
#[derive(Debug, Clone)]
pub struct DiscV5WithV4Downgrade<T = MustIncludeChain> {
    discv5: Arc<DiscV5<T>>,
    discv4: Discv4,
}

impl<T> DiscV5WithV4Downgrade<T> {
    /// Returns a new combined handle.
    pub fn new(discv5: Arc<DiscV5<T>>, discv4: Discv4) -> Self {
        Self { discv5, discv4 }
    }

    /// Exposes methods on [`Discv4`] that take a reference to self.
    pub fn with_discv4<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Discv4) -> R,
    {
        f(&self.discv4)
    }

    /// Exposes methods on [`Discv4`] that take a mutable reference to self.
    pub fn with_discv4_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Discv4) -> R,
    {
        f(&mut self.discv4)
    }

    /// Starts [`discv5::Discv5`] as primary service, and [`Discv4`] as downgraded service.
    pub async fn start(
        discv4_addr: SocketAddr, // discv5 addr in config
        sk: SecretKey,
        discv4_config: Discv4Config,
        discv5_config: DiscV5Config<T>,
    ) -> Result<(Self, MergedUpdateStream, NodeRecord), Error>
    where
        T: FilterDiscovered + Send + Sync + Clone + 'static,
    {
        // todo: verify not same socket discv4 and 5

        //
        // 1. start discv5
        //
        let (discv5, discv5_updates, bc_local_discv5_enr) =
            DiscV5::start(&sk, discv5_config).await?;

        //
        // 2. types needed for interfacing with discv4
        //
        let discv5 = Arc::new(discv5);
        let discv5_ref = discv5.clone();
        // todo: store peer ids as node ids also in discv4 + pass mutual ref to mirror as
        // param to filter out removed nodes and only get peer ids of additions.
        let read_kbuckets_callback = move || -> Result<HashSet<PeerId>, secp256k1::Error> {
            let keys = discv5_ref.with_kbuckets(|kbuckets| {
                kbuckets
                    .read()
                    .iter_ref()
                    .map(|node| {
                        let enr = node.node.value;
                        let pk = enr.public_key();
                        debug_assert!(
                            matches!(pk, discv5::enr::CombinedPublicKey::Secp256k1(_)),
                            "discv5 using different key type than discv4"
                        );
                        pk.encode()
                    })
                    .collect::<Vec<_>>()
            });

            let mut discv5_kbucket_keys = HashSet::with_capacity(keys.len());

            for pk_bytes in keys {
                let pk = PublicKey::from_slice(&pk_bytes)?;
                let peer_id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
                discv5_kbucket_keys.insert(peer_id);
            }

            Ok(discv5_kbucket_keys)
        };
        // channel which will tell discv4 that discv5 has updated its kbuckets
        let (discv5_kbuckets_change_tx, discv5_kbuckets_change_rx) = watch::channel(());

        //
        // 4. start discv4 as discv5 fallback, maintains a mirror of discv5 kbuckets
        //
        let local_enr_discv4 = NodeRecord::from_secret_key(discv4_addr, &sk);

        let (discv4, mut discv4_service) = match Discv4::bind_as_secondary_disc_node(
            discv4_addr,
            local_enr_discv4,
            sk,
            discv4_config,
            discv5_kbuckets_change_rx,
            read_kbuckets_callback,
        )
        .await
        {
            Ok(discv4) => discv4,
            Err(err) => return Err(Error::Discv4InitFailure(err)),
        };

        // start an update stream
        let discv4_updates = discv4_service.update_stream();

        // spawn the service
        let _discv4_service = discv4_service.spawn();

        //
        // 5. merge both discovery nodes
        //
        // combined handle
        let disc = DiscV5WithV4Downgrade::new(discv5, discv4);

        // combined update stream
        let disc_updates = MergedUpdateStream::merge_discovery_streams(
            discv5_updates,
            discv4_updates,
            discv5_kbuckets_change_tx,
        );

        // discv5 and discv4 are running like usual, only that discv4 will filter out
        // nodes already connected over discv5 identified by their public key
        Ok((disc, disc_updates, bc_local_discv5_enr))
    }
}

impl<T> HandleDiscovery for DiscV5WithV4Downgrade<T> {
    fn add_node_to_routing_table(
        &self,
        node_record: NodeFromExternalSource,
    ) -> Result<(), impl std::error::Error> {
        self.discv5.add_node(node_record).map_err(Error::DiscV5Error)
    }

    fn set_eip868_in_local_enr(&self, key: Vec<u8>, rlp: Bytes) {
        self.discv5.set_eip868_in_local_enr(key.clone(), rlp.clone());
        self.discv4.set_eip868_in_local_enr(key, rlp)
    }

    fn encode_and_set_eip868_in_local_enr(&self, key: Vec<u8>, value: impl alloy_rlp::Encodable) {
        let mut buf = BytesMut::new();
        value.encode(&mut buf);
        self.set_eip868_in_local_enr(key, buf.freeze())
    }

    fn ban_peer_by_ip_and_node_id(&self, peer_id: PeerId, ip: IpAddr) {
        self.discv5.ban_peer_by_ip_and_node_id(peer_id, ip);
        self.discv4.ban_peer_by_ip_and_node_id(peer_id, ip)
    }

    fn ban_peer_by_ip(&self, ip: IpAddr) {
        self.discv5.ban_peer_by_ip(ip);
        self.discv4.ban_peer_by_ip(ip)
    }

    fn node_record(&self) -> NodeRecord {
        self.discv5.node_record()
    }
}

impl<T> HandleDiscv5 for DiscV5WithV4Downgrade<T>
where
    T: FilterDiscovered,
{
    type Filter = T;

    fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DiscV5<Self::Filter>) -> R,
    {
        f(&self.discv5)
    }

    fn ip_mode(&self) -> IpMode {
        self.discv5.ip_mode()
    }

    fn fork_id_key(&self) -> &[u8] {
        self.discv5.fork_id_key()
    }

    fn filter_discovered_peer(&self, enr: &discv5::Enr) -> FilterOutcome
    where
        Self::Filter: FilterDiscovered,
    {
        self.discv5.filter_discovered_peer(enr)
    }
}

/// Wrapper around update type used in [`discv5::Discv5`] and [`Discv4`].
#[derive(Debug, From)]
pub enum DiscoveryUpdateV5 {
    /// A [`discv5::Event`], an update from [`discv5::Discv5`].
    V5(discv5::Event),
    /// A [`DiscoveryUpdate`], an update from [`Discv4`].
    V4(DiscoveryUpdate),
}

/// Stream wrapper for streams producing types that can convert to [`DiscoveryUpdateV5`].
#[derive(Debug)]
pub struct UpdateStream<S>(S);

impl<S, I> Stream for UpdateStream<S>
where
    S: Stream<Item = I> + Unpin,
    DiscoveryUpdateV5: From<I>,
{
    type Item = DiscoveryUpdateV5;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(ready!(self.0.poll_next_unpin(cx)).map(DiscoveryUpdateV5::from))
    }
}

/// A stream that polls update streams from [`discv5::Discv5`] and [`Discv4`] in round-robin
/// fashion.
#[derive(Debug)]
pub struct MergedUpdateStream {
    inner: Select<
        UpdateStream<ReceiverStream<discv5::Event>>,
        UpdateStream<ReceiverStream<DiscoveryUpdate>>,
    >,
    discv5_kbuckets_change_tx: watch::Sender<()>,
}

impl MergedUpdateStream {
    /// Returns a merged stream of [`discv5::Event`]s and [`DiscoveryUpdate`]s, that supports
    /// downgrading to discv4.
    pub fn merge_discovery_streams(
        discv5_event_stream: mpsc::Receiver<discv5::Event>,
        discv4_update_stream: ReceiverStream<DiscoveryUpdate>,
        discv5_kbuckets_change_tx: watch::Sender<()>,
    ) -> Self {
        let discv5_event_stream = UpdateStream(ReceiverStream::new(discv5_event_stream));
        let discv4_update_stream = UpdateStream(discv4_update_stream);

        Self { inner: select(discv5_event_stream, discv4_update_stream), discv5_kbuckets_change_tx }
    }

    /// Notifies [`Discv4`] that [discv5::Discv5]'s kbuckets have been updated and that [`Discv4`]
    /// should update its mirror thereof.
    fn notify_discv4_of_kbuckets_update(&self) -> Result<(), watch::error::SendError<()>> {
        self.discv5_kbuckets_change_tx.send(())
    }
}

impl Stream for MergedUpdateStream {
    type Item = DiscoveryUpdateV5; // todo: return result

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let update = ready!(self.inner.poll_next_unpin(cx));
        if let Some(DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(ref enr, _))) = update {
            //
            // Notify discv4 that a discv5 session has been established.
            //
            // A peer with a WAN reachable socket, is likely to make it into discv5 kbuckets
            // shortly after session establishment. Manually added nodes (e.g. from dns service)
            // won't emit a `discv5::Event::NodeInserted`, hence we use the
            // `discv5::Event::SessionEstablished` event + check the enr for contactable address,
            // to determine if discv4 should be notified.
            //
            if discv5::IpMode::DualStack.get_contactable_addr(enr).is_none() {
                cx.waker().wake_by_ref();
                return Poll::Pending
            }
            // todo: get clarity on rules on fork id in discv4
            // todo: check discv4s policy for peers with non-WAN-reachable node records.
            // todo: upstream discovered discv4 peers to ping from discv5?

            if let Err(err) = self.notify_discv4_of_kbuckets_update() {
                error!(target: "net::discv5::downgrade_v4",
                    %err,
                    "failed to notify discv4 of discv5 kbuckets update",
                );
            }
        }

        Poll::Ready(update)
    }
}

#[cfg(test)]
mod tests {
    use discv5::enr::Enr;
    use rand::thread_rng;
    use reth_discv4::Discv4ConfigBuilder;
    use tracing::trace;

    use crate::{enr::EnrCombinedKeyWrapper, filter::NoopFilter};

    use super::*;

    async fn start_discovery_node(
        udp_port_discv4: u16,
        udp_port_discv5: u16,
    ) -> (DiscV5WithV4Downgrade<NoopFilter>, MergedUpdateStream, NodeRecord) {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv4_addr = format!("127.0.0.1:{udp_port_discv4}").parse().unwrap();
        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        // disable `NatResolver`
        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = DiscV5Config::builder()
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .filter(NoopFilter)
            .build();

        DiscV5WithV4Downgrade::start(discv4_addr, secret_key, discv4_config, discv5_config)
            .await
            .expect("should build discv5 with discv4 downgrade")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discv5_with_discv4_downgrade() {
        reth_tracing::init_test_tracing();

        // rig test

        // rig node_1
        let (node_1, mut stream_1, discv4_enr_1) = start_discovery_node(31314, 31324).await;
        let discv5_enr_node_1 = node_1.with_discv5(|discv5| discv5.local_enr());

        let discv4_id_1 = discv4_enr_1.id;
        let discv5_id_1 = discv5_enr_node_1.node_id();

        // rig node_2
        let (node_2, mut stream_2, discv4_enr_2) = start_discovery_node(32324, 32325).await;
        let discv5_enr_node_2 = node_2.with_discv5(|discv5| discv5.local_enr());

        let discv4_id_2 = discv4_enr_2.id;
        let discv5_id_2 = discv5_enr_node_2.node_id();

        trace!(target: "net::discv5::v4_downgrade::tests",
            node_1_node_id=format!("{:#}", discv5_id_1),
            node_2_node_id=format!("{:#}", discv5_id_2),
            "started nodes"
        );

        // test

        // add node_2 manually to node_1:discv4 kbuckets
        node_1.with_discv4(|discv4| {
            discv4
                .add_node_to_routing_table(NodeFromExternalSource::NodeRecord(discv4_enr_2))
                .unwrap()
        });

        // verify node_2 is in KBuckets of node_1:discv4 and vv
        let event_1_v4 = stream_1.next().await.unwrap();
        let event_2_v4 = stream_2.next().await.unwrap();
        matches!(
            event_1_v4,
            DiscoveryUpdateV5::V4(DiscoveryUpdate::Added(node)) if node == discv4_enr_2
        );
        matches!(
            event_2_v4,
            DiscoveryUpdateV5::V4(DiscoveryUpdate::Added(node)) if node == discv4_enr_1
        );

        // add node_2 to discovery handle of node_1 (should add node to discv5 kbuckets)
        let discv5_enr_node_2_reth_compatible_ty: Enr<SecretKey> =
            EnrCombinedKeyWrapper(discv5_enr_node_2.clone()).into();
        node_1
            .add_node_to_routing_table(NodeFromExternalSource::Enr(
                discv5_enr_node_2_reth_compatible_ty,
            ))
            .unwrap();
        // verify node_2 is in KBuckets of node_1:discv5
        assert!(node_1.with_discv5(|discv5| discv5.table_entries_id().contains(&discv5_id_2)));

        // manually trigger connection from node_1 to node_2
        node_1.with_discv5(|discv5| discv5.send_ping(discv5_enr_node_2.clone())).await.unwrap();

        // verify node_1:discv5 is connected to node_2:discv5 and vv
        let event_2_v5 = stream_2.next().await.unwrap();
        let event_1_v5 = stream_1.next().await.unwrap();
        matches!(
            event_1_v5,
            DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(node, socket)) if node == discv5_enr_node_2 && socket == discv5_enr_node_2.udp4_socket().unwrap().into()
        );
        matches!(
            event_2_v5,
            DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(node, socket)) if node == discv5_enr_node_1 && socket == discv5_enr_node_1.udp4_socket().unwrap().into()
        );

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = stream_2.next().await.unwrap();
        matches!(
            event_2_v5,
            DiscoveryUpdateV5::V5(discv5::Event::NodeInserted { node_id, replaced }) if node_id == discv5_id_2 && replaced.is_none()
        );

        let event_2_v4 = stream_2.next().await.unwrap();
        let event_1_v4 = stream_1.next().await.unwrap();
        matches!(
            event_1_v4,
            DiscoveryUpdateV5::V4(DiscoveryUpdate::Removed(node_id)) if node_id == discv4_id_2
        );
        matches!(
            event_2_v4,
            DiscoveryUpdateV5::V4(DiscoveryUpdate::Removed(node_id)) if node_id == discv4_id_1
        );
    }
}
