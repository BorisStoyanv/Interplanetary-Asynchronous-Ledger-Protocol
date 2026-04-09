#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ialp_summary_exporter::run_cli().await
}
