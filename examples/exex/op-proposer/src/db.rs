use std::str::FromStr;

use reth_primitives::B256;
use reth_tracing::tracing::info;
use rusqlite::Connection;

use crate::L2Output;

pub struct L2OutputDb {
    connection: Connection,
}

impl L2OutputDb {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }
    pub fn initialize(&mut self) -> eyre::Result<()> {
        // Create tables to store L2 outputs
        self.connection.execute(
            r#"
                    CREATE TABLE IF NOT EXISTS l2Outputs (
                        l2_block_number     INTEGER NOT NULL PRIMARY KEY,
                        output_root         TEXT NOT NULL,
                        l1_block_hash       TEXT NOT NULL UNIQUE,
                        l1_block_number     INTEGER NOT NULL
                    );
                "#,
            (),
        )?;
        info!("Initialized l2Outputs table");

        Ok(())
    }

    pub fn get_l2_output(&self, l2_block_number: u64) -> eyre::Result<L2Output> {
        let l2_output = self.connection.query_row(
            r#"
            SELECT * FROM l2Outputs WHERE l2_block_number = ?
            "#,
            (l2_block_number,),
            |row| {
                let l2_block_number = row.get::<_, u64>(0)?;
                let output_root = B256::from_str(&row.get::<_, String>(1)?).unwrap();
                let l1_block_hash = B256::from_str(&row.get::<_, String>(2)?).unwrap();
                let l1_block_number = row.get::<_, u64>(3)?;

                Ok(L2Output { output_root, l2_block_number, l1_block_hash, l1_block_number })
            },
        )?;

        Ok(l2_output)
    }

    pub fn insert_l2_output(&mut self, l2_output: L2Output) -> eyre::Result<()> {
        self.connection.execute(
            r#"
            INSERT INTO l2Outputs (l2_block_number, output_root, l1_block_hash, l1_block_number)
            VALUES (?, ?, ?, ?)
            "#,
            (
                l2_output.l2_block_number,
                l2_output.output_root.to_string(),
                l2_output.l1_block_hash.to_string(),
                l2_output.l1_block_number,
            ),
        )?;
        Ok(())
    }

    pub fn delete_l2_output(&mut self, l2_block_number: u64) -> eyre::Result<()> {
        self.connection
            .execute("DELETE FROM l2Outputs WHERE l2_block_number = ?;", (l2_block_number,))?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use alloy_primitives::FixedBytes;

    use super::L2OutputDb;

    pub fn setup() -> eyre::Result<L2OutputDb> {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = L2OutputDb::new(connection);
        db.initialize().unwrap();
        Ok(db)
    }

    #[test]
    fn test_insert_and_get_l2_output() -> eyre::Result<()> {
        let mut db = setup()?;
        let l2_output = crate::L2Output {
            output_root: FixedBytes::default(),
            l2_block_number: 1,
            l1_block_hash: FixedBytes::default(),
            l1_block_number: 2,
        };

        db.insert_l2_output(l2_output.clone())?;
        let result = db.get_l2_output(1)?;
        assert_eq!(result, l2_output);
        Ok(())
    }

    #[test]
    fn test_delete_l2_output() -> eyre::Result<()> {
        let mut db = setup()?;
        let l2_output = crate::L2Output {
            output_root: FixedBytes::default(),
            l2_block_number: 1,
            l1_block_hash: FixedBytes::default(),
            l1_block_number: 2,
        };

        db.insert_l2_output(l2_output.clone())?;
        db.delete_l2_output(1)?;
        let result = db.get_l2_output(1);
        assert!(result.is_err());
        Ok(())
    }
}
