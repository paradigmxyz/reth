//! Dynamically loaded ExEx loading test
use reth_exex::dyexex::DyExExLoader;
use reth_exex_test_utils::test_exex_context;

const PATH_TO_MINIMAL_EXEX: &str = "tests/assets/libminimal_exex.dylib";

#[tokio::test]
async fn should_load_symbol() -> eyre::Result<()> {
    let (ctx, _) = test_exex_context().await?;
    let dyn_ctx = ctx.into_dyn();

    let mut loader = DyExExLoader::new();
    loader.load(PATH_TO_MINIMAL_EXEX, dyn_ctx)?;

    let (exex_id, _launch_fn) =
        loader.loaded.pop().expect("expect a loaded dylib").into_id_and_launch_fn();
    assert_eq!(exex_id, "minimal_exex");

    Ok(())
}
