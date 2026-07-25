#[tokio::main]
async fn main() -> anyhow::Result<()> {
    toposaic_api::run().await
}
