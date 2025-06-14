//! Rollkit node binary with standard reth CLI support and custom Engine API functionality.
//!
//! This node supports all standard reth CLI flags and functionality, with a customized
//! `engine_forkchoiceUpdatedV3` method that uses the rollkit payload builder for handling
//! transactions passed through the Engine API.

#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rlp::Decodable;
use alloy_rpc_types::{
    engine::{
        ExecutionData, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
        ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5, ExecutionPayloadV1,
        PayloadAttributes as EthPayloadAttributes, PayloadId,
    },
    Withdrawal,
};
use clap::Parser;
use reth_payload_builder::{BuildArguments, BuildOutcome, PayloadBuilder, PayloadConfig};
use reth_ethereum_cli::Cli;
use reth_engine_local::payload::UnsupportedLocalAttributes;
use reth_ethereum::{
    chainspec::{ChainSpec, ChainSpecProvider},
    node::{
        api::{
            payload::{EngineApiMessageVersion, EngineObjectValidationError, PayloadOrAttributes},
            validate_version_specific_fields, AddOnsContext, EngineTypes, EngineValidator,
            FullNodeComponents, FullNodeTypes, InvalidPayloadAttributesError, NewPayloadError,
            NodeTypes, PayloadAttributes, PayloadBuilderAttributes, PayloadTypes, PayloadValidator,
        },
        builder::{
            components::{BasicPayloadServiceBuilder, ComponentsBuilder, PayloadBuilderBuilder},
            rpc::{EngineValidatorBuilder, RpcAddOns},
            BuilderContext, Node, NodeAdapter, NodeComponentsBuilder,
        },
        node::{
            EthereumConsensusBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder,
            EthereumPoolBuilder,
        },
        EthEvmConfig, EthereumEthApiBuilder,
    },
    pool::{PoolTransaction, TransactionPool},
    primitives::{RecoveredBlock, SealedBlock},
    TransactionSigned,
};
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;

use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes, PayloadBuilderError};
use reth_revm::cached::CachedReads;
use reth_trie_db::MerklePatriciaTrie;
use rollkit_payload_builder::{
    PayloadAttributesError, RollkitPayloadAttributes, RollkitPayloadBuilder,
    RollkitPayloadBuilderConfig,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tracing::info;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator(); 