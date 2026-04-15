#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ialp_scenario_harness::run_cli().await
}
