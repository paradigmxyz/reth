use clap::Parser;

#[derive(Debug, Parser)]
pub struct InitCommand;

impl InitCommand {
    pub async fn execute(&self) -> eyre::Result<()> {
        todo!()
    }
}
