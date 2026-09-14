//! Prints a deterministic system-test trace as a readable timeline.

use reth_dst::DecisionTrace;
use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let program = arguments.next().unwrap_or_default();
    let Some(path) = arguments.next() else {
        eprintln!("usage: {} <trace.dst>", std::path::Path::new(&program).display());
        return ExitCode::FAILURE
    };
    if arguments.next().is_some() {
        eprintln!("expected exactly one trace path");
        return ExitCode::FAILURE
    }

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("failed to read {}: {error}", std::path::Path::new(&path).display());
            return ExitCode::FAILURE
        }
    };
    let trace = match DecisionTrace::decode(&bytes) {
        Ok(trace) => trace,
        Err(error) => {
            eprintln!("failed to decode {}: {error}", std::path::Path::new(&path).display());
            return ExitCode::FAILURE
        }
    };

    println!("schema: {}", trace.header.schema_version);
    println!("source: {}", trace.header.source_revision);
    println!("seed: {:?}", trace.header.exploration_seed);
    println!("budget: {:?}", trace.header.decision_budget);
    println!("configuration: {}", trace.header.configuration_digest);
    println!("genesis: {}", trace.header.genesis_digest);
    println!("decisions: {}", trace.decisions.len());
    for (index, decision) in trace.decisions.iter().enumerate() {
        println!(
            "{index}: {:?}/{}#{}@{} option={} {}",
            decision.point.domain,
            decision.point.actor,
            decision.point.occurrence,
            decision.virtual_time,
            decision.option.0,
            decision.summary
        );
    }
    ExitCode::SUCCESS
}
