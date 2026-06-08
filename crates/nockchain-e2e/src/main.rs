use std::path::PathBuf;
use std::time::Duration;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Parser, Subcommand};
use nockchain_e2e::{peer_speedup, runner, sizing};
use nockchain_types::tx_engine::common::Hash;

#[derive(Parser, Debug)]
#[command(name = "nockchain-e2e")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run {
        scenario: PathBuf,
        #[arg(long)]
        nockchain_bin: Option<PathBuf>,
        #[arg(long)]
        docker_image: Option<String>,
        #[arg(long, default_value_t = false)]
        docker: bool,
        #[arg(long)]
        wallet_bin: Option<PathBuf>,
        #[arg(
            long,
            action = ArgAction::Set,
            value_parser = BoolishValueParser::new(),
            default_value_t = true,
            help = "Use target/release/nockchain when --nockchain-bin is not provided"
        )]
        release: bool,
        #[arg(long, default_value = "target/nockchain-e2e")]
        work_dir: PathBuf,
        #[arg(long, default_value_t = 6100)]
        base_grpc_port: u16,
        #[arg(long, default_value_t = 7100)]
        base_private_grpc_port: u16,
        #[arg(long, default_value_t = 4100)]
        base_p2p_port: u16,
        #[arg(long, default_value_t = false)]
        keep_artifacts: bool,
    },
    BlockSizeReport {
        #[arg(long)]
        server: String,
        #[arg(long)]
        tx_id: String,
        #[arg(long)]
        selected_note_count: u64,
        #[arg(long)]
        total_input_nicks: u64,
        #[arg(long)]
        fee_nicks: u64,
        #[arg(long)]
        send_amount_nicks: u64,
        #[arg(long, default_value_t = 180)]
        wait_seconds: u64,
        #[arg(long, default_value_t = 1_048_576)]
        gen2_batch_max_bytes: u64,
        #[arg(long, default_value_t = 131_072)]
        gen2_item_max_bytes: u64,
    },
    AssertPeerSpeedup {
        #[arg(long = "server", required = true)]
        servers: Vec<String>,
        #[arg(long, default_value_t = 5_000)]
        sample_interval_ms: u64,
        #[arg(long, default_value_t = 1.10)]
        min_speedup_ratio: f64,
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
    WaitForPublicHeight {
        #[arg(long)]
        server: String,
        #[arg(long)]
        height: u64,
        #[arg(long, default_value_t = 900)]
        timeout_seconds: u64,
    },
    #[command(name = "wait-for-demo-live", alias = "wait-for-review-ready")]
    WaitForDemoLive {
        #[arg(long)]
        server: String,
        #[arg(long, default_value_t = 1200)]
        timeout_seconds: u64,
    },
    WaitForSeedCatchUp {
        #[arg(long)]
        server: String,
        #[arg(long = "reference-server")]
        reference_server: String,
        #[arg(long, default_value_t = 1200)]
        timeout_seconds: u64,
    },
    /// Sweep a height range on a running node and report the distribution of
    /// `block_raw_bytes + sum(tx_size_bytes)` per block — the quantity that
    /// determines whether an atomic block-plus-txs bundle fits inside a Gen2
    /// batch response. Writes a JSON report to stdout by default.
    BlockTxDistribution {
        #[arg(long)]
        server: String,
        #[arg(long)]
        start_height: u64,
        #[arg(long)]
        end_height: u64,
        #[arg(long, default_value_t = 16)]
        concurrency: usize,
        #[arg(long, default_value_t = 4_194_304)]
        gen2_batch_max_bytes: u64,
        #[arg(long, default_value_t = 2_097_152)]
        gen2_block_batch_max_response_bytes: u64,
        #[arg(long, default_value_t = 2_097_152)]
        gen2_item_max_bytes: u64,
        /// Optional path to write the full report (including per-block samples) as JSON.
        /// When set, stdout receives only the summary block for human reading.
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            scenario,
            nockchain_bin,
            docker_image,
            docker,
            wallet_bin,
            release,
            work_dir,
            base_grpc_port,
            base_private_grpc_port,
            base_p2p_port,
            keep_artifacts,
        } => {
            let docker_image = if docker {
                docker_image.or_else(|| std::env::var("NOCKCHAIN_E2E_IMAGE").ok())
            } else {
                None
            };
            let nockchain_bin = if docker {
                None
            } else {
                nockchain_bin.or_else(|| {
                    Some(if release {
                        PathBuf::from("target/release/nockchain")
                    } else {
                        PathBuf::from("target/debug/nockchain")
                    })
                })
            };
            if !docker {
                if let Some(path) = &nockchain_bin {
                    if !path.exists() {
                        return Err(anyhow::anyhow!(
                            "nockchain binary not found at {} (build it or set --nockchain-bin/--release=false)",
                            path.display()
                        ));
                    }
                }
            }
            if let Some(path) = &wallet_bin {
                if !path.exists() {
                    return Err(anyhow::anyhow!(
                        "wallet binary not found at {} (build it or set --wallet-bin)",
                        path.display()
                    ));
                }
            }
            let options = runner::RunOptions {
                scenario_path: scenario,
                nockchain_bin,
                wallet_bin,
                work_dir,
                base_grpc_port,
                base_private_grpc_port,
                base_p2p_port,
                docker,
                docker_image,
                keep_artifacts,
            };
            runner::run_scenario(options).await
        }
        Command::BlockSizeReport {
            server,
            tx_id,
            selected_note_count,
            total_input_nicks,
            fee_nicks,
            send_amount_nicks,
            wait_seconds,
            gen2_batch_max_bytes,
            gen2_item_max_bytes,
        } => {
            let tx_id = Hash::from_base58(&tx_id)
                .map_err(|err| anyhow::anyhow!("invalid --tx-id '{}': {}", tx_id, err))?;
            let summary = sizing::build_fan_in_block_size_summary(
                &server,
                &tx_id,
                selected_note_count,
                total_input_nicks,
                fee_nicks,
                send_amount_nicks,
                Duration::from_secs(wait_seconds),
                gen2_batch_max_bytes,
                gen2_item_max_bytes,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        Command::AssertPeerSpeedup {
            servers,
            sample_interval_ms,
            min_speedup_ratio,
            json_out,
        } => {
            peer_speedup::assert_peer_speedup(peer_speedup::AssertPeerSpeedupOptions {
                servers,
                sample_interval: Duration::from_millis(sample_interval_ms),
                min_speedup_ratio,
                json_out,
            })
            .await?;
            Ok(())
        }
        Command::WaitForPublicHeight {
            server,
            height,
            timeout_seconds,
        } => {
            let head = nockchain_e2e::grpc::wait_for_height(
                &server,
                height,
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!("ready: {} reached height {}", server, head.height);
            Ok(())
        }
        Command::WaitForDemoLive {
            server,
            timeout_seconds,
        } => {
            let demo_live = nockchain_e2e::grpc::wait_for_demo_live(
                &server,
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!(
                "demo-live: {} peers={} cache={} heaviest={} local_gap={} refresh_successes={} backfill_successes={}",
                server,
                demo_live.peer_count,
                demo_live.cache_height,
                demo_live.heaviest_height,
                demo_live
                    .heaviest_height
                    .saturating_sub(demo_live.cache_height),
                demo_live.refresh_success_count,
                demo_live.backfill_success_count
            );
            Ok(())
        }
        Command::WaitForSeedCatchUp {
            server,
            reference_server,
            timeout_seconds,
        } => {
            let catch_up = nockchain_e2e::grpc::wait_for_seed_catch_up(
                &server,
                &reference_server,
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!(
                "seed-caught-up: {} reached {} cache={} target={} heaviest={} local_gap={} peers={}",
                server,
                reference_server,
                catch_up.local.cache_height,
                catch_up.reference_cache_height,
                catch_up.local.heaviest_height,
                catch_up
                    .local
                    .heaviest_height
                    .saturating_sub(catch_up.local.cache_height),
                catch_up.local.peer_count
            );
            Ok(())
        }
        Command::BlockTxDistribution {
            server,
            start_height,
            end_height,
            concurrency,
            gen2_batch_max_bytes,
            gen2_block_batch_max_response_bytes,
            gen2_item_max_bytes,
            json_out,
        } => {
            let report = sizing::collect_block_tx_distribution(
                &server, start_height, end_height, concurrency, gen2_batch_max_bytes,
                gen2_block_batch_max_response_bytes, gen2_item_max_bytes,
            )
            .await?;

            if let Some(path) = json_out {
                let payload = serde_json::to_vec_pretty(&report)?;
                std::fs::write(&path, &payload).map_err(|err| {
                    anyhow::anyhow!("failed to write {}: {}", path.display(), err)
                })?;
                print_distribution_summary(&report);
                eprintln!("wrote full report to {}", path.display());
            } else {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            Ok(())
        }
    }
}

fn print_distribution_summary(report: &sizing::BlockTxDistributionReport) {
    fn line(label: &str, d: &sizing::DistributionStats) {
        if d.count == 0 {
            println!("{label:30} (no samples)");
            return;
        }
        println!(
            "{label:30} count={:<6} min={:<10} p50={:<10} p90={:<10} p95={:<10} p99={:<10} max={:<10} mean={:<10}",
            d.count, d.min, d.p50, d.p90, d.p95, d.p99, d.max, d.mean,
        );
    }
    println!("range: {}..={}", report.range_start, report.range_end);
    println!(
        "requested={} observed={} not_found={} blocks_with_missing_tx_data={}",
        report.requested_heights,
        report.blocks_observed,
        report.heights_not_found,
        report.blocks_with_missing_tx_data,
    );
    line("block_raw_bytes", &report.block_raw_bytes);
    line("tx_bytes_sum_per_block", &report.tx_bytes_sum_per_block);
    line("combined_per_block", &report.combined_per_block);
    line("individual_tx_bytes", &report.individual_tx_bytes);
    line("tx_count_per_block", &report.tx_count_per_block);
    println!(
        "cap pressure: {} blocks > gen2_block_batch cap ({}), {} blocks > gen2_batch max ({})",
        report.blocks_over_block_batch_cap,
        report.gen2_block_batch_max_response_bytes,
        report.blocks_over_gen2_batch_cap,
        report.gen2_batch_max_bytes,
    );
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command};

    #[test]
    fn wait_for_demo_live_parses_new_command_name() {
        let cli = Cli::try_parse_from([
            "nockchain-e2e", "wait-for-demo-live", "--server", "127.0.0.1:50053",
        ])
        .expect("new demo-live command should parse");

        match cli.command {
            Command::WaitForDemoLive {
                server,
                timeout_seconds,
            } => {
                assert_eq!(server, "127.0.0.1:50053");
                assert_eq!(timeout_seconds, 1_200);
            }
            other => panic!("expected WaitForDemoLive, got {other:?}"),
        }
    }

    #[test]
    fn wait_for_review_ready_alias_still_parses_demo_live_command() {
        let cli = Cli::try_parse_from([
            "nockchain-e2e", "wait-for-review-ready", "--server", "127.0.0.1:50053",
            "--timeout-seconds", "45",
        ])
        .expect("legacy review-ready alias should keep working");

        match cli.command {
            Command::WaitForDemoLive {
                server,
                timeout_seconds,
            } => {
                assert_eq!(server, "127.0.0.1:50053");
                assert_eq!(timeout_seconds, 45);
            }
            other => panic!("expected WaitForDemoLive alias, got {other:?}"),
        }
    }

    #[test]
    fn wait_for_seed_catch_up_parses_reference_server() {
        let cli = Cli::try_parse_from([
            "nockchain-e2e", "wait-for-seed-catch-up", "--server", "127.0.0.1:50053",
            "--reference-server", "127.0.0.1:50051", "--timeout-seconds", "600",
        ])
        .expect("seed catch-up command should parse");

        match cli.command {
            Command::WaitForSeedCatchUp {
                server,
                reference_server,
                timeout_seconds,
            } => {
                assert_eq!(server, "127.0.0.1:50053");
                assert_eq!(reference_server, "127.0.0.1:50051");
                assert_eq!(timeout_seconds, 600);
            }
            other => panic!("expected WaitForSeedCatchUp, got {other:?}"),
        }
    }
}
