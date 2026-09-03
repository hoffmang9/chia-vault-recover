use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chia_vault_recover::chain::ChainClient;
use chia_vault_recover::config::VaultConfig;
use chia_vault_recover::keys::MnemonicWordCount;
use chia_vault_recover::network::{Backend, Network};
use chia_vault_recover::recovery::VaultPhase;
use chia_vault_recover::workflow::{self, LookupReport, StartWorkflow};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "chia-vault-recover",
    version,
    about = "Recover a Chia Cloud Wallet vault from its receive address"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Look up a vault from its receive address (start here)
    #[command(visible_alias = "discover", visible_alias = "resolve")]
    Lookup {
        /// Vault Receive address (`xch1…` / `txch1…`). A hex launcher id also works.
        #[arg(long, alias = "address", alias = "launcher-id")]
        vault: String,
        #[arg(long, env = "CHIA_VAULT_RECOVERY_MNEMONIC")]
        recovery_mnemonic: Option<String>,
        #[arg(long)]
        recovery_mnemonic_file: Option<PathBuf>,
        /// Clawback timelock in seconds. If omitted, common Cloud Wallet values are tried.
        #[arg(long)]
        clawback_secs: Option<u64>,
        #[arg(long, default_value = "mainnet")]
        network: NetworkArg,
        #[command(flatten)]
        backend: BackendArgs,
        #[arg(long, default_value = "vault-config.json")]
        out_config: PathBuf,
    },
    /// Start delayed recovery (signs with the Cloud Wallet recovery phrase)
    Start {
        /// Vault Receive address. Looks up the vault and rebuilds layout if `--config` is omitted.
        #[arg(long, alias = "address")]
        vault: Option<String>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, env = "CHIA_VAULT_RECOVERY_MNEMONIC")]
        recovery_mnemonic: Option<String>,
        #[arg(long)]
        recovery_mnemonic_file: Option<PathBuf>,
        #[arg(long, env = "CHIA_VAULT_NEW_CUSTODY_MNEMONIC")]
        new_custody_mnemonic: Option<String>,
        #[arg(long)]
        new_custody_mnemonic_file: Option<PathBuf>,
        #[arg(long)]
        new_recovery_mnemonic: Option<String>,
        #[arg(long)]
        new_recovery_mnemonic_file: Option<PathBuf>,
        #[arg(long, default_value = "24")]
        word_count: u8,
        #[arg(long)]
        new_clawback_secs: Option<u64>,
        #[arg(long)]
        clawback_secs: Option<u64>,
        #[arg(long, default_value = "mainnet")]
        network: NetworkArg,
        #[command(flatten)]
        backend: BackendArgs,
        #[arg(long, default_value = "post-recovery-vault-config.json")]
        out_config: PathBuf,
        /// Where to write the rebuilt public layout when starting from `--vault`.
        #[arg(long, default_value = "vault-config.json")]
        lookup_config: PathBuf,
    },
    /// Finish delayed recovery after the clawback timelock
    Finish {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        post_recovery_config: PathBuf,
        #[arg(long, default_value = "mainnet")]
        network: NetworkArg,
        #[command(flatten)]
        backend: BackendArgs,
    },
    /// Verify a vault-config JSON against the on-chain singleton
    Inspect {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "mainnet")]
        network: NetworkArg,
        #[command(flatten)]
        backend: BackendArgs,
        #[arg(long)]
        post_recovery_config: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, ValueEnum)]
enum NetworkArg {
    Mainnet,
    Testnet11,
}

impl From<NetworkArg> for Network {
    fn from(value: NetworkArg) -> Self {
        match value {
            NetworkArg::Mainnet => Network::Mainnet,
            NetworkArg::Testnet11 => Network::Testnet11,
        }
    }
}

#[derive(Clone, Debug, clap::Args)]
struct BackendArgs {
    #[arg(long, default_value = "coinset")]
    backend: BackendKind,
    #[arg(long)]
    full_node_url: Option<String>,
}

#[derive(Clone, Debug, ValueEnum)]
enum BackendKind {
    Coinset,
    Rpc,
}

impl BackendArgs {
    fn into_backend(self) -> Result<Backend> {
        match self.backend {
            BackendKind::Coinset => Ok(Backend::Coinset),
            BackendKind::Rpc => Ok(Backend::FullNode {
                url: self
                    .full_node_url
                    .context("--full-node-url is required with --backend rpc")?,
            }),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Lookup {
            vault,
            recovery_mnemonic,
            recovery_mnemonic_file,
            clawback_secs,
            network,
            backend,
            out_config,
        } => {
            let (client, network) = client_for_vault(&vault, network, backend)?;
            let mnemonic = optional_mnemonic(recovery_mnemonic, recovery_mnemonic_file)?;
            let report =
                workflow::lookup(&client, &vault, mnemonic.as_deref(), clawback_secs).await?;
            print_lookup(&report, network, Some(&out_config))?;
            if matches!(report, LookupReport::NeedFallback { .. }) {
                std::process::exit(2);
            }
        }
        Commands::Start {
            vault,
            config,
            recovery_mnemonic,
            recovery_mnemonic_file,
            new_custody_mnemonic,
            new_custody_mnemonic_file,
            new_recovery_mnemonic,
            new_recovery_mnemonic_file,
            word_count,
            new_clawback_secs,
            clawback_secs,
            network,
            backend,
            out_config,
            lookup_config,
        } => {
            let recovery_mnemonic =
                read_mnemonic(recovery_mnemonic, recovery_mnemonic_file, "recovery")?;
            let (client, network, config) = match (vault, config) {
                (Some(vault), _) => {
                    let (client, network) = client_for_vault(&vault, network, backend)?;
                    let report =
                        workflow::lookup(&client, &vault, Some(&recovery_mnemonic), clawback_secs)
                            .await?;
                    print_lookup(&report, network, Some(&lookup_config))?;
                    let LookupReport::Reconstructed(discovered) = report else {
                        bail!("cannot start recovery until lookup rebuilds the vault layout");
                    };
                    (client, network, discovered.config)
                }
                (None, Some(path)) => {
                    let network = Network::from(network);
                    let client = ChainClient::new(network, &backend.into_backend()?);
                    (client, network, VaultConfig::load(&path)?)
                }
                (None, None) => {
                    bail!("pass --vault <xch1…> (recommended) or --config <vault-config.json>")
                }
            };
            let new_custody_mnemonic = read_mnemonic(
                new_custody_mnemonic,
                new_custody_mnemonic_file,
                "new custody",
            )?;
            let new_recovery_mnemonic = match (new_recovery_mnemonic, new_recovery_mnemonic_file) {
                (None, None) => None,
                (m, f) => Some(read_mnemonic(m, f, "new recovery")?),
            };
            let word_count = match word_count {
                12 => MnemonicWordCount::Words12,
                24 => MnemonicWordCount::Words24,
                _ => bail!("word_count must be 12 or 24"),
            };

            let result = workflow::start(
                &client,
                StartWorkflow {
                    config: &config,
                    recovery_mnemonic: &recovery_mnemonic,
                    new_custody_mnemonic: &new_custody_mnemonic,
                    new_recovery_mnemonic: new_recovery_mnemonic.as_deref(),
                    new_clawback_timelock: new_clawback_secs,
                    new_word_count: word_count,
                    network,
                    out_config: &out_config,
                },
            )
            .await?;

            println!("pushed start recovery spend");
            println!(
                "wrote public post-recovery config: {}",
                out_config.display()
            );
            println!(
                "clawback_timelock_secs: {} — wait, then run finish",
                result.clawback_timelock
            );
            if let Some(words) = result.generated_recovery_mnemonic {
                println!();
                println!(
                    "*** SAVE THIS NEW RECOVERY MNEMONIC (shown once, not written to config) ***"
                );
                println!("{words}");
                println!("***");
            }
        }
        Commands::Finish {
            config,
            post_recovery_config,
            network,
            backend,
        } => {
            let network = Network::from(network);
            let client = ChainClient::new(network, &backend.into_backend()?);
            let config = VaultConfig::load(&config)?;
            let post = VaultConfig::load(&post_recovery_config)?;
            let handle = workflow::finish(&client, &config, &post, network).await?;
            println!("pushed finish recovery spend (handle): {handle}");
        }
        Commands::Inspect {
            config,
            network,
            backend,
            post_recovery_config,
        } => {
            let client = ChainClient::new(network.into(), &backend.into_backend()?);
            let config = VaultConfig::load(&config)?;
            let post = post_recovery_config
                .as_ref()
                .map(VaultConfig::load)
                .transpose()?;
            let report = workflow::inspect(&client, &config, post.as_ref()).await?;
            println!("launcher_id: 0x{}", hex::encode(report.launcher_id));
            println!(
                "expected_ready_ph: 0x{}",
                hex::encode(report.expected_ready_puzzle_hash)
            );
            match report.on_chain_puzzle_hash {
                Some(ph) => println!("on_chain_ph: 0x{}", hex::encode(ph)),
                None => println!("on_chain_ph: (not found)"),
            }
            println!("phase: {:?}", report.phase);
            println!("clawback_timelock_secs: {}", report.clawback_timelock);
            println!("{}", report.guidance);
            if report.phase == VaultPhase::InRecovery {
                std::process::exit(2);
            }
        }
    }
    Ok(())
}

fn client_for_vault(
    vault: &str,
    network: NetworkArg,
    backend: BackendArgs,
) -> Result<(ChainClient, Network)> {
    let locator = chia_vault_recover::parse_vault_locator(vault)
        .context("invalid --vault (expected xch1…/txch1… Receive address or 0x launcher id)")?;
    let network = locator.inferred_network().unwrap_or(network.into());
    Ok((ChainClient::new(network, &backend.into_backend()?), network))
}

fn print_lookup(
    report: &LookupReport,
    network: Network,
    out_config: Option<&std::path::Path>,
) -> Result<()> {
    println!("network: {}", network.as_str());
    match report {
        LookupReport::Reconstructed(discovered) => {
            if let Some(path) = out_config {
                discovered.config.save(path)?;
                println!("wrote vault config: {}", path.display());
            }
            println!(
                "launcher_id: 0x{}",
                hex::encode(discovered.config.launcher_id_bytes()?)
            );
            println!("resolved_from: {}", discovered.launcher_source);
            println!("custody_hash: 0x{}", hex::encode(discovered.custody_hash));
            println!("clawback_timelock_secs: {}", discovered.clawback_timelock);
            println!(
                "current_coin: 0x{}",
                hex::encode(discovered.current_coin.coin_id())
            );
            if discovered.members_complete {
                println!("custody members: parsed from spend");
            } else {
                println!(
                    "custody members: hash only (M-of-N or unparsed); enough for delayed recovery"
                );
            }
            println!("{}", discovered.guidance);
        }
        LookupReport::ReadyForPhrase {
            launcher_id,
            launcher_source,
            guidance,
        } => {
            println!("launcher_id: 0x{}", hex::encode(launcher_id));
            println!("resolved_from: {launcher_source}");
            println!("{guidance}");
            println!(
                "Re-run lookup with --recovery-mnemonic-file (or CHIA_VAULT_RECOVERY_MNEMONIC)."
            );
        }
        LookupReport::NeedFallback {
            launcher_id,
            launcher_source,
            reason,
            guidance,
        } => {
            if let Some(id) = launcher_id {
                println!("launcher_id: 0x{}", hex::encode(id));
            }
            if let Some(source) = launcher_source {
                println!("resolved_from: {source}");
            }
            println!("{reason}");
            println!();
            println!("{guidance}");
        }
    }
    Ok(())
}

fn optional_mnemonic(inline: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    match (inline, file) {
        (None, None) => Ok(None),
        (m, f) => Ok(Some(read_mnemonic(m, f, "recovery")?)),
    }
}

fn read_mnemonic(inline: Option<String>, file: Option<PathBuf>, label: &str) -> Result<String> {
    if let Some(path) = file {
        return Ok(std::fs::read_to_string(path)?.trim().to_string());
    }
    inline.with_context(|| format!("{label} mnemonic required (--*-mnemonic or --*-mnemonic-file)"))
}
