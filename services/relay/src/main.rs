#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ialp_summary_relay::run_cli().await
}
