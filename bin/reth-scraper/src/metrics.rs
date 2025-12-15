use metrics::{counter, describe_counter, describe_gauge, gauge};

pub fn describe_metrics() {
    describe_counter!("scraper_nodes_discovered", "Total nodes discovered via discv4");
    describe_counter!("scraper_handshakes_success", "Successful P2P handshakes");
    describe_counter!("scraper_handshakes_failed", "Failed P2P handshakes");
    describe_counter!("scraper_recheck_success", "Successful node rechecks");
    describe_counter!("scraper_recheck_failed", "Failed node rechecks");
    describe_gauge!("scraper_unique_nodes", "Number of unique nodes in database");
    describe_gauge!("scraper_alive_nodes", "Number of alive nodes in database");
    describe_gauge!("scraper_active_workers", "Number of active handshake workers");
}

pub fn inc_discovered() {
    counter!("scraper_nodes_discovered").increment(1);
}

pub fn inc_handshake_success() {
    counter!("scraper_handshakes_success").increment(1);
}

pub fn inc_handshake_failed() {
    counter!("scraper_handshakes_failed").increment(1);
}

pub fn set_unique_nodes(n: u64) {
    gauge!("scraper_unique_nodes").set(n as f64);
}

pub fn set_active_workers(n: u64) {
    gauge!("scraper_active_workers").set(n as f64);
}

pub fn inc_recheck_success() {
    counter!("scraper_recheck_success").increment(1);
}

pub fn inc_recheck_failed() {
    counter!("scraper_recheck_failed").increment(1);
}

pub fn set_alive_nodes(n: u64) {
    gauge!("scraper_alive_nodes").set(n as f64);
}
