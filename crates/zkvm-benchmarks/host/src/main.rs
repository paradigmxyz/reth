//! Benchmarking harness

mod generate_witness;
use generate_witness::generate;
use sp1_sdk::{ProverClient, SP1Stdin};
use std::collections::HashMap;

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const STATELESS_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../target/elf-compilation/riscv32im-succinct-zkvm-elf/release/stateless-program"
));
// /Users/kev/work/ethereum/reth/target/
/// Cycle count metrics for a particular workload
#[derive(Debug)]
pub struct WorkloadMetrics {
    /// Name of the workload
    name: String,
    /// Total number of cycles
    total_num_cycles: u64,
    /// Region specific cycles
    region_cycles: HashMap<String, u64>,
}

fn main() {
    // Setup the logger.
    // sp1_sdk::utils::setup_logger();
    dotenv::dotenv().ok();

    // Setup the prover client.
    let client = ProverClient::from_env();

    let mut reports = Vec::new();
    for blockchain_corpus in generate().into_iter() {
        // Iterate each block in the mini blockchain
        let name = blockchain_corpus.name.clone();
        for client_input in blockchain_corpus.blocks_and_witnesses.iter().take(3) {
            let block_number = client_input.block.number;
            let mut stdin = SP1Stdin::new();
            stdin.write(client_input);
            stdin.write(&blockchain_corpus.network);

            let (_, report) = client.execute(STATELESS_ELF, &stdin).run().unwrap();

            let total_num_cycles = report.total_instruction_count();
            let region_cycles: HashMap<_, _> = report.cycle_tracker.into_iter().collect();

            let metrics = WorkloadMetrics {
                name: format!("{}-{}", name, block_number),
                total_num_cycles,
                region_cycles,
            };
            reports.push(metrics);
        }
        // Print out the reports to std for now
        // We can prettify it later.
        dbg!(&reports);
    }
}
