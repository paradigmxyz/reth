use reth_primitives::ChainSpec;

fn main() {
    let chain = ChainSpec::mainnet();
    println!("Ethereum Mainnet Chain ID: {}", chain.chain().id());
}
