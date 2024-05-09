use alloy_primitives::{Address, Bytes, B256};
use alloy_sol_types::sol;
use reth_node_ethereum::EthereumNode;

sol! {
    #[sol(rpc)]
    contract L2OutputOracle {
        function proposeL2Output(bytes32 _outputRoot, uint256 _l2BlockNumber, bytes32 _l1BlockHash, uint256 _l1BlockNumber) external payable;
        function latestBlockNumber() public view returns (uint256);
    }
    #[sol(rpc)]
    contract OptimismPortal {
        struct OutputRootProof {
            bytes32 version;
            bytes32 stateRoot;
            bytes32 messagePasserStorageRoot;
            bytes32 latestBlockhash;
        }

        struct WithdrawalTransaction {
            uint256 nonce;
            address sender;
            address target;
            uint256 value;
            uint256 gasLimit;
            bytes data;
        }
        function proveWithdrawalTransaction(WithdrawalTransaction memory _tx, uint256 _l2OutputIndex, OutputRootProof calldata _outputRootProof,bytes[] calldata _withdrawalProof) external;
    }
}

// ProvenWithdrawalParameters is the set of parameters to pass to the ProveWithdrawalTransaction
// and FinalizeWithdrawalTransaction functions
pub struct ProofWithdrawalParameters {
    nonce: u64,
    sender: Address,
    target: Address,
    value: u64,
    gas_limit: u64,
    l2_output_index: u64,
    data: Bytes,
    output_root_proof: OutputRootProof,
    withdrawal_proof: Vec<Bytes>,
}

pub struct OutputRootProof {
    version: Bytes,
    state_root: B256,
    message_passer_storage_root: B256,
    latest_blockhash: B256,
}

pub struct L2Output {
    output_root: B256,
    l2_block_number: u64,
    l1_block_hash: B256,
    l1_block_number: u64,
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            // .install_exex("OpProposer", |ctx| async move { todo!("implement here") })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
