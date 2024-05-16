
#[tokio::test]
async fn test_should_import_data_from_evm() {

    // Step 1: Prepare the data
    let evm_datasource_url = "https://orca-app-5yyst.ondigitalocean.app";
    let () super::utils::temp_data(evm_datasource_url).await.unwrap();

    // Step 2: Import the data
    bitfinity_import_it::import_evm_data(&provider_factory, &blockchain_db).await.unwrap();

    // Step 3: Verify the data
    // let chain = provider_factory.chain();
    // let chain_id = chain.chain_id();
    // let chain_name = chain.chain_name();
    // let chain_spec = chain.chain_spec();
    // let chain_genesis_hash = chain.genesis_hash();
    // let chain_genesis_block = chain.genesis_block();
    // let chain_latest_block = chain.latest_block();



}
