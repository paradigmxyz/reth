pub static HYPERLIQUID: LazyLock<Arc<ChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/hyperliquid.json"))
        .expect("Can't deserialize Hyperliquid genesis json");
    let hardforks = EthereumHardfork::hyperliquid().into();
    let mut spec = ChainSpec {
        chain: Chain::hyperliquid(),
        genesis_header: SealedHeader::new(
            make_genesis_header(&genesis, &hardforks),
            HYPERLIQUID_GENESIS_HASH,
        ),
        genesis,
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks,
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: 10000,
        ..Default::default()
    };
    spec.into()
});

pub static HYPERLIQUID_ENGINE_CAPABILITIES: &[&str] = &[
    "engine_forkchoiceUpdatedV1",
    "engine_getPayloadV1",
    "engine_newPayloadV1",
    "engine_getClientVersionV1",
];

impl EngineApi<HyperliquidEngineTypes> {
    pub fn hyperliquid_config() -> Self {
        Self::new(
            provider.clone(),
            chain_spec.clone(),
            beacon_engine_handle,
            payload_store,
            pool,
            task_executor,
            client,
            EngineCapabilities::new(HYPERLIQUID_ENGINE_CAPABILITIES.iter().copied()),
            validator,
            false,
        )
    }
}

impl RpcServerArgs {
    pub fn hyperliquid_config() -> Self {
        Self {
            auth_addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            auth_port: 8551,
            auth_jwtsecret: Some(PathBuf::from("hyperliquid/jwt.hex")),
            auth_ipc: true,
            auth_ipc_path: "/tmp/hyperliquid_engine_api.ipc".to_string(),
            ..Default::default()
        }
    }
}

impl OpEngineValidator {
    pub fn hyperliquid_config() -> Self {
        Self::new(
            chain_spec.clone(),
            false, // accept_execution_requests_hash
            false, // enable_tx_conditional
        )
    }
}

impl NetworkConfig {
    pub fn hyperliquid_config() -> Self {
        Self::default()
            .with_network_mode(NetworkMode::Stake)
            .with_chain_id(ChainId::from(1337)) // HyperliquidのチェーンID
            .with_disable_discv4_discovery()
            .with_disable_discv5_discovery()
    }
}

#[derive(Debug, Default, Clone)]
pub struct HyperliquidEngineTypes;

impl EngineTypes for HyperliquidEngineTypes {
    type ExecutionData = ExecutionData;
    type BuiltPayload = HyperliquidBuiltPayload;
    type PayloadAttributes = HyperliquidPayloadAttributes;
    type PayloadBuilderAttributes = HyperliquidPayloadBuilderAttributes;
}

#[derive(Debug, Clone)]
pub struct HyperliquidBuiltPayload {
    pub block: SealedBlock<Block>,
    pub fees: U256,
}

impl BuiltPayload for HyperliquidBuiltPayload {
    type Primitives = HyperliquidPrimitives;
    type Block = Block;
    
    fn block(&self) -> &SealedBlock<Self::Block> {
        &self.block
    }
    
    fn fees(&self) -> U256 {
        self.fees
    }
}

impl TracingConfig {
    pub fn hyperliquid_config() -> Self {
        Self {
            log_level: "info".to_string(),
            log_file: Some(PathBuf::from("hyperliquid.log")),
            log_format: LogFormat::Json,
            ..Default::default()
        }
    }
}

impl EngineApiMetrics {
    pub fn hyperliquid_config() -> Self {
        Self {
            new_payload_latency: Gauge::new("hyperliquid_engine_new_payload_latency", "Latency of new payload processing"),
            forkchoice_updated_latency: Gauge::new("hyperliquid_engine_forkchoice_updated_latency", "Latency of forkchoice updates"),
            get_payload_latency: Gauge::new("hyperliquid_engine_get_payload_latency", "Latency of get payload requests"),
        }
    }
}
