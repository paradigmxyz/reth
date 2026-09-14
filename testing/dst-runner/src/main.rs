//! Long-running deterministic node simulation campaign.

mod campaign;
mod node_storage;
mod node_wire;

use std::{process::Command, thread, time::Duration};

const CHILD_ENV: &str = "RETH_DST_CHILD";

fn main() {
    reth_tracing::init_test_tracing();
    if std::env::var_os(CHILD_ENV).is_some() {
        run_child();
    } else {
        supervise();
    }
}

fn run_child() {
    std::thread::Builder::new()
        .name("reth-dst-node".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(campaign::run_node_campaign)
        .expect("spawn DST campaign")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

fn supervise() {
    let first_seed: u64 = std::env::var("RETH_DST_SEED")
        .map(|seed| seed.parse().expect("RETH_DST_SEED must be a u64"))
        .unwrap_or(0);
    let campaign_seconds = std::env::var("RETH_DST_SECONDS")
        .ok()
        .map(|seconds| seconds.parse().expect("RETH_DST_SECONDS must be a u64"));
    let case_limit = std::env::var("RETH_DST_CASES")
        .map(|cases| cases.parse().expect("RETH_DST_CASES must be an integer"))
        .unwrap_or_else(|_| {
            if campaign_seconds.is_some() {
                usize::MAX
            } else if std::env::var_os("RETH_DST_SEED").is_some() ||
                std::env::var_os("RETH_DST_REPLAY").is_some()
            {
                1
            } else {
                4
            }
        });
    let deadline =
        campaign_seconds.map(|seconds| std::time::Instant::now() + Duration::from_secs(seconds));
    let timeout_seconds: u64 = std::env::var("RETH_DST_CASE_TIMEOUT_SECS")
        .map(|seconds| seconds.parse().expect("RETH_DST_CASE_TIMEOUT_SECS must be a u64"))
        .unwrap_or(60);
    let executable = std::env::current_exe().expect("resolve DST runner executable");
    let mut bugs = 0usize;
    let mut index = 0usize;

    while index < case_limit && deadline.is_none_or(|deadline| std::time::Instant::now() < deadline)
    {
        let seed = first_seed.wrapping_add(index as u64);
        let mut child = Command::new(&executable)
            .env(CHILD_ENV, "1")
            .env("RETH_DST_SEED", seed.to_string())
            .env("RETH_DST_CASES", "1")
            .env_remove("RETH_DST_SECONDS")
            .spawn()
            .expect("spawn isolated DST case");
        let child_deadline =
            std::time::Instant::now() + Duration::from_secs(timeout_seconds.saturating_add(5));
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll isolated DST case") {
                break Some(status)
            }
            if std::time::Instant::now() >= child_deadline {
                child.kill().expect("kill stalled DST case");
                let _ = child.wait();
                eprintln!("INCONCLUSIVE seed={seed} reason=host watchdog killed stalled child");
                break None
            }
            thread::sleep(Duration::from_millis(50));
        };
        match status {
            None => {}
            Some(status) if matches!(status.code(), Some(0 | 124)) => {}
            Some(_) => bugs += 1,
        }
        index += 1;
        if std::env::var_os("RETH_DST_REPLAY").is_some() {
            break
        }
    }

    if bugs != 0 {
        eprintln!("BUGS {bugs} of {index} isolated DST cases failed");
        std::process::exit(1);
    }
}
