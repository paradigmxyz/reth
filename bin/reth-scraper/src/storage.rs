use crate::types::NodeInfo;
use alloy_primitives::B512;
use reth_network_peers::NodeRecord;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::{
    net::IpAddr,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Storage {
    pub(crate) pool: SqlitePool,
}

impl Storage {
    pub async fn new(path: &str) -> eyre::Result<Self> {
        let url = format!("sqlite:{}?mode=rwc", path);
        let pool = SqlitePoolOptions::new().max_connections(5).connect(&url).await?;
        Ok(Self { pool })
    }

    pub async fn init_schema(&self) -> eyre::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                enode TEXT NOT NULL,
                ip TEXT NOT NULL,
                tcp_port INTEGER NOT NULL,
                udp_port INTEGER NOT NULL,
                client_version TEXT NOT NULL,
                capabilities TEXT NOT NULL,
                eth_version INTEGER,
                chain_id INTEGER,
                country_code TEXT,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                last_error TEXT,
                last_checked INTEGER,
                is_alive INTEGER NOT NULL DEFAULT 1,
                consecutive_failures INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Add columns if they don't exist (for existing databases)
        let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN last_checked INTEGER")
            .execute(&self.pool)
            .await;
        let _ =
            sqlx::query("ALTER TABLE nodes ADD COLUMN is_alive INTEGER NOT NULL DEFAULT 1")
                .execute(&self.pool)
                .await;
        let _ = sqlx::query(
            "ALTER TABLE nodes ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&self.pool)
        .await;

        Ok(())
    }

    pub async fn upsert_node(&self, node: &NodeInfo) -> eyre::Result<()> {
        let node_id = format!("{:?}", node.node_id);
        let capabilities = node.capabilities_string();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        sqlx::query(
            r#"
            INSERT INTO nodes (node_id, enode, ip, tcp_port, udp_port, client_version, capabilities, eth_version, chain_id, country_code, first_seen, last_seen, last_error, last_checked, is_alive, consecutive_failures)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1, 0)
            ON CONFLICT(node_id) DO UPDATE SET
                enode = excluded.enode,
                ip = excluded.ip,
                tcp_port = excluded.tcp_port,
                udp_port = excluded.udp_port,
                client_version = excluded.client_version,
                capabilities = excluded.capabilities,
                eth_version = excluded.eth_version,
                chain_id = excluded.chain_id,
                country_code = excluded.country_code,
                last_seen = excluded.last_seen,
                last_error = excluded.last_error,
                last_checked = excluded.last_checked,
                is_alive = 1,
                consecutive_failures = 0
            "#,
        )
        .bind(&node_id)
        .bind(&node.enode)
        .bind(node.ip.to_string())
        .bind(node.tcp_port as i32)
        .bind(node.udp_port as i32)
        .bind(&node.client_version)
        .bind(&capabilities)
        .bind(node.eth_version.map(|v| v as i32))
        .bind(node.chain_id.map(|v| v as i64))
        .bind(&node.country_code)
        .bind(now)
        .bind(now)
        .bind(&node.last_error)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get nodes that need to be rechecked (oldest `last_checked` first)
    pub async fn get_stale_nodes(
        &self,
        limit: u32,
        max_age_secs: u64,
    ) -> eyre::Result<Vec<StaleNode>> {
        let cutoff =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 - max_age_secs as i64;

        let rows: Vec<(String, String, i32, i32)> = sqlx::query_as(
            r#"
            SELECT node_id, ip, tcp_port, udp_port 
            FROM nodes 
            WHERE is_alive = 1 AND (last_checked IS NULL OR last_checked < ?1)
            ORDER BY last_checked ASC NULLS FIRST
            LIMIT ?2
            "#,
        )
        .bind(cutoff)
        .bind(limit as i32)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .filter_map(|(node_id, ip, tcp_port, udp_port)| {
                Some(StaleNode {
                    node_id: node_id.parse().ok()?,
                    ip: ip.parse().ok()?,
                    tcp_port: tcp_port as u16,
                    udp_port: udp_port as u16,
                })
            })
            .collect::<Vec<_>>()
            .pipe(Ok)
    }

    /// Update a node after successful recheck
    pub async fn update_node_alive(
        &self,
        node_id: &B512,
        client_version: &str,
        country_code: Option<&str>,
    ) -> eyre::Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let node_id_str = format!("{:?}", node_id);

        sqlx::query(
            r#"
            UPDATE nodes SET 
                client_version = ?1,
                country_code = ?2,
                last_seen = ?3,
                last_checked = ?3,
                is_alive = 1,
                consecutive_failures = 0,
                last_error = NULL
            WHERE node_id = ?4
            "#,
        )
        .bind(client_version)
        .bind(country_code)
        .bind(now)
        .bind(&node_id_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Increment failure count for a node, mark dead after threshold
    pub async fn increment_failure(
        &self,
        node_id: &B512,
        error: &str,
        max_failures: u32,
    ) -> eyre::Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let node_id_str = format!("{:?}", node_id);

        sqlx::query(
            r#"
            UPDATE nodes SET 
                last_checked = ?1,
                last_error = ?2,
                consecutive_failures = consecutive_failures + 1,
                is_alive = CASE WHEN consecutive_failures + 1 >= ?3 THEN 0 ELSE 1 END
            WHERE node_id = ?4
            "#,
        )
        .bind(now)
        .bind(error)
        .bind(max_failures as i32)
        .bind(&node_id_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    pub async fn get_all_nodes(&self) -> eyre::Result<Vec<NodeInfo>> {
        type NodeRow = (
            String,
            String,
            String,
            i32,
            i32,
            String,
            String,
            Option<i32>,
            Option<i64>,
            Option<String>,
            i64,
            i64,
            Option<String>,
            Option<i64>,
            i32,
            i32,
        );
        let rows: Vec<NodeRow> = sqlx::query_as(
            "SELECT node_id, enode, ip, tcp_port, udp_port, client_version, capabilities, eth_version, chain_id, country_code, first_seen, last_seen, last_error, last_checked, is_alive, consecutive_failures FROM nodes",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(NodeInfo {
                    node_id: row.0.parse().unwrap_or_default(),
                    enode: row.1,
                    ip: row.2.parse()?,
                    tcp_port: row.3 as u16,
                    udp_port: row.4 as u16,
                    client_version: row.5,
                    capabilities: row.6.split(',').map(String::from).collect(),
                    eth_version: row.7.map(|v| v as u8),
                    chain_id: row.8.map(|v| v as u64),
                    country_code: row.9,
                    first_seen: row.10 as u64,
                    last_seen: row.11 as u64,
                    last_error: row.12,
                    last_checked: row.13.map(|v| v as u64),
                    is_alive: row.14 != 0,
                    consecutive_failures: row.15 as u32,
                })
            })
            .collect()
    }

    pub async fn get_stats_by_version(&self) -> eyre::Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT client_version, COUNT(*) as count FROM nodes WHERE is_alive = 1 GROUP BY client_version ORDER BY count DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_stats_by_country(&self) -> eyre::Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT COALESCE(country_code, 'Unknown') as country, COUNT(*) as count FROM nodes WHERE is_alive = 1 GROUP BY country_code ORDER BY count DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn count_nodes(&self) -> eyre::Result<i64> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM nodes")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn count_alive_nodes(&self) -> eyre::Result<i64> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM nodes WHERE is_alive = 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }
}

/// A node that needs to be rechecked
#[derive(Debug, Clone)]
pub struct StaleNode {
    pub node_id: B512,
    pub ip: IpAddr,
    pub tcp_port: u16,
    pub udp_port: u16,
}

impl StaleNode {
    pub const fn to_node_record(&self) -> NodeRecord {
        NodeRecord {
            address: self.ip,
            tcp_port: self.tcp_port,
            udp_port: self.udp_port,
            id: self.node_id,
        }
    }
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}

impl<T> Pipe for T {}
