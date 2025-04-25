use ef_tests::{
    cases::blockchain_test::{run_case, BlockchainTestCase},
    Case,
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

use reth_stateless::{fork_spec::ForkSpec, ClientInput};

/// Name of a directory in the ethereum spec tests
const VALID_BLOCKS: &str = "ValidBlocks";

/// You can think of a test case as a mini-blockchain
pub struct BlocksAndWitnesses {
    /// Name of the blockchain test
    pub name: String,
    /// Block coupled with its corresponding execution witness
    pub blocks_and_witnesses: Vec<ClientInput>,
    /// Chain specification
    // TODO: Don't think we want to pass this through maybe ForkSpec
    // TODO: Also Genesis file is wrong in chainspec
    // TODO: We can keep this initially and don't measure the time it takes to deserialize
    pub network: ForkSpec,
}

// This method will fetch all tests in the ethereum-tests/Blockchaintests/ValidBlocks folder
// and generate a stateless witness for them.
pub(crate) fn generate() -> Vec<BlocksAndWitnesses> {
    // First get the path to "ValidBlocks"
    let suite_path = path_to_ef_tests_suite(VALID_BLOCKS);

    // Verify that the path exists
    assert!(suite_path.exists(), "Test suite path does not exist: {suite_path:?}");

    // Find all files with the ".json" extension in the test suite directory
    // Each Json file corresponds to a BlockchainTestCase
    let test_cases: Vec<_> = find_all_files_with_extension(&suite_path, ".json")
        .into_iter()
        .map(|test_case_path| {
            let case = BlockchainTestCase::load(&test_case_path).expect("test case should load");
            (test_case_path, case)
        })
        .collect();

    let mut blocks_and_witnesses = Vec::new();
    for (_, test_case) in test_cases.into_iter() {
        if test_case.skip {
            continue;
        }
        let blockchain_case: Vec<BlocksAndWitnesses> = test_case
            // Inside of a JSON file, we can have multiple tests, for example testopcode_Cancun,
            // testopcode_Prague
            // This is why we have `tests`.
            .tests
            .par_iter()
            .filter(|(_, case)| !BlockchainTestCase::excluded_fork(case.network))
            .map(|(name, case)| BlocksAndWitnesses {
                name: name.to_string(),
                blocks_and_witnesses: run_case(case)
                    .unwrap()
                    .into_iter()
                    .map(|(block, witness)| ClientInput { block, witness })
                    .collect(),
                network: case.network.into(),
            })
            .collect();
        blocks_and_witnesses.extend(blockchain_case);
    }

    blocks_and_witnesses
}

// This function was copied from `ef-tests`
/// Recursively find all files with a given extension.
fn find_all_files_with_extension(path: &Path, extension: &str) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(extension))
        .map(DirEntry::into_path)
        .collect()
}

// Path to the ethereum-tests in the ef-tests crate
fn path_to_ef_tests_suite(suite: &str) -> PathBuf {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("should be at the zkvm-benchmarks dir")
        .parent()
        .expect("should be at the crates directory")
        .parent()
        .expect("should be at the workspace directory");

    workspace_root
        .join("testing")
        .join("ef-tests")
        .join("ethereum-tests")
        .join("BlockchainTests")
        .join(suite)
}
