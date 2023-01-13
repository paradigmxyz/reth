use clap::Parser;
use eyre::WrapErr;
use reth_primitives::Block;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct ImportCommand {
    /// The file to import.
    path: PathBuf,
}

impl ImportCommand {
    pub async fn execute(&self) -> eyre::Result<()> {
        // read file
        let mut contents = &std::fs::read(&self.path).wrap_err("Could not open file")?[..];

        // open or create db
        let mut imported = 0;
        while !contents.is_empty() {
            match <Block as reth_rlp::Decodable>::decode(&mut contents) {
                Ok(block) => {
                    println!("{:?}", block.header.hash_slow());
                    imported += 1;
                }
                Err(err) => {
                    println!("Error: {:?}", err);
                    break
                }
            }
            // do what headers does
            // do what bodies does
            // 1. check if we have the block
            // 2. if we do, error out
            // 3. check if the block connects
            // 4. if not, error out
            // 5. otherwise, import the block
        }

        // execute all stages

        println!("Imported {imported} blocks");
        Ok(())
    }
}

#[derive(Debug, Parser)]
pub struct InitCommand;

impl InitCommand {
    pub async fn execute(&self) -> eyre::Result<()> {
        todo!()
    }
}
