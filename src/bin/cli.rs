use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chia_vault_recover::chain::ChainClient;
use chia_vault_recover::config::VaultConfig;
use chia_vault_recover::keys::MnemonicWordCount;
use chia_vault_recover::network::{Backend, Network};
use chia_vault_recover::recovery::VaultPhase;
use chia_vault_recover::workflow::{self, StartWorkflow};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "chia-vault-recover",
    version,
    about = "Recover a Chia Cloud Wallet vault"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Verify vault-config against the on-chain singleton and print guidance
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
    /// Start delayed recovery (signs with the Cloud Wallet recovery phrase)
    Start {
        #[arg(long)]
        config: PathBuf,
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
        #[arg(long, default_value = "mainnet")]
        network: NetworkArg,
        #[command(flatten)]
        backend: BackendArgs,
        #[arg(long, default_value = "post-recovery-vault-config.json")]
        out_config: PathBuf,
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
        Commands::Start {
            config,
            recovery_mnemonic,
            recovery_mnemonic_file,
            new_custody_mnemonic,
            new_custody_mnemonic_file,
            new_recovery_mnemonic,
            new_recovery_mnemonic_file,
            word_count,
            new_clawback_secs,
            network,
            backend,
            out_config,
        } => {
            let network = Network::from(network);
            let client = ChainClient::new(network, &backend.into_backend()?);
            let config = VaultConfig::load(&config)?;
            let recovery_mnemonic =
                read_mnemonic(recovery_mnemonic, recovery_mnemonic_file, "recovery")?;
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
    }
    Ok(())
}

fn read_mnemonic(inline: Option<String>, file: Option<PathBuf>, label: &str) -> Result<String> {
    if let Some(path) = file {
        return Ok(std::fs::read_to_string(path)?.trim().to_string());
    }
    inline.with_context(|| format!("{label} mnemonic required (--*-mnemonic or --*-mnemonic-file)"))
}
