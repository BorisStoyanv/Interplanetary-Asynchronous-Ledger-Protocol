#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ialp_summary_importer::run_cli().await
}
