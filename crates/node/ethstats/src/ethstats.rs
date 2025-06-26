//! Support ethstats, the network stats reporting service

use crate::{connection::ConnWrapper, error::Error};
use tokio::{select, time};

use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{timeout, Duration, interval};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message, WebSocketStream};
use url::Url;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;

use ethstats_events::{AuthMsg, NodeInfo, StatsMsg};

pub fn parse_ethstats_url(url: &str) -> Result<(String, String, String), String> {
    if let Some((pre_host, host)) = url.rsplit_once('@') {
        if let Some((nodename, pass)) = pre_host.rsplit_once(':') {
            return Ok((nodename.to_string(), pass.to_string(), host.to_string()));
        } else {
            return Ok((pre_host.to_string(), "".to_string(), host.to_string()));
        }
    }
    Err(format!("Invalid ethstats URL: {}", url))
}

pub struct EthStatsService {
    node_id: String,
    secret: String,
    host: String,
    conn: Option<ConnWrapper>,
    shutdown_rx: mpsc::Receiver<()>,
}

impl EthStatsService {
    pub async fn new(url: &str, shutdown_rx: mpsc::Receiver<()>) -> Result<Self, String> {
        let (node, secret, host) = parse_ethstats_url(url)?;
        let full_url = format!("ws://{}/api", host);
        let url = Url::parse(&full_url).map_err(|e| e.to_string())?;

        match connect_async(url).await {
            Ok((ws_stream, _)) => {
                let conn = ConnWrapper::new(ws_stream);
                let mut service = EthStatsService {
                    node_id: node,
                    secret,
                    host,
                    conn: Some(conn),
                    shutdown_rx,
                };
                service.login().await?;
                Ok(service)
            }
            Err(e) => Err(format!("Connection failed: {}", e)),
        }
    }

    async fn login(&mut self) -> Result<(), String> {
        let conn = self.conn.as_ref().unwrap();

        let auth = AuthMsg {
            id: self.node_id.clone(),
            secret: self.secret.clone(),
            info: NodeInfo {
                name: self.node_id.clone(),
                node: "rust-client".into(),
                port: 30303,
                network: "1".into(),
                protocol: "eth/66".into(),
                api: "No".into(),
                os: std::env::consts::OS.into(),
                os_ver: std::env::consts::ARCH.into(),
                client: "0.1.0".into(),
                history: true,
            },
        };

        let message = auth.generate_login_message();
        conn.write_json(&message).await.map_err(|e| e.to_string())?;

        let response = timeout(Duration::from_secs(5), conn.read_json()).await
            .map_err(|_| "Timeout waiting for login ack".to_string())?
            .map_err(|e| e.to_string())?;

        if let Some(ack) = response.get("emit") {
            if ack.get(0) == Some(&Value::String("ready".to_string())) {
                return Ok(());
            }
        }

        Err("Unauthorized or unexpected login response".to_string())
    }

    async fn report_stats(&self) {
        if let Some(conn) = &self.conn {
            let stats_msg = StatsMsg {
                id: self.node_id.clone(),
                active: true,
                syncing: false,
                peers: 5,
                gas_price: 123456,
                uptime: 100,
            };
            let payload = stats_msg.to_message();
            if let Err(e) = conn.write_json(&payload).await {
                eprintln!("Failed to send stats: {}", e);
            }
        }
    }

    pub async fn run_loop(&mut self) {
        let mut tick = interval(Duration::from_secs(15));
        let conn_clone = self.conn.clone();

        // Spawn goroutine equivalent: async reader
        if let Some(conn) = conn_clone {
            let node_id = self.node_id.clone();
            let conn_write = conn.clone();

            tokio::spawn(async move {
                loop {
                    match conn.read_json().await {
                        Ok(msg) => {
                            if let Some(emit) = msg.get("emit") {
                                if let Some(Value::String(command)) = emit.get(0) {
                                    match command.as_str() {
                                        "node-pong" => {
                                            println!("[{}] pong received", node_id);
                                        },
                                        "history" => {
                                            println!("[{}] history requested", node_id);
                                            let dummy = json!({"emit": ["history", {"id": node_id, "history": []}]});
                                            if let Err(e) = conn_write.write_json(&dummy.to_string()).await {
                                                eprintln!("[{}] failed to respond to history: {}", node_id, e);
                                            }
                                        },
                                        other => println!("[{}] unhandled: {}", node_id, other),
                                    }
                                }
                            }
                        },
                        Err(e) => {
                            eprintln!("[{}] read error: {}", node_id, e);
                            break;
                        }
                    }
                }
            });
        }

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.report_stats().await;
                },
                Some(_) = self.shutdown_rx.recv() => {
                    println!("Received shutdown signal");
                    break;
                }
            }
        }
    }
}
