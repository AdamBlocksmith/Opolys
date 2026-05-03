//! Opolys wallet CLI — `opl`
//!
//! Command-line wallet for creating accounts, signing transactions,
//! querying the chain via RPC, and submitting transactions.
//!
//! Usage:
//!   opl new                    — Generate a new 24-word mnemonic
//!   opl balance <address>      — Query account balance via RPC
//!   opl transfer <to> <amt>   — Create and sign a transfer transaction
//!   opl bond <amount>           — Create and sign a refiner bond
//!   opl unbond <amount>         — Create and sign a refiner unbond
//!   opl send <tx_hex>          — Broadcast a signed transaction via RPC

use clap::{Args, Parser};
use opolys_core::{FLAKES_PER_OPL, FlakeAmount, MAINNET_CHAIN_ID};
use opolys_wallet::{Bip39Mnemonic, TransactionSigner};

const DEFAULT_RPC_URL: &str = "https://localhost:4171";

#[derive(Parser, Debug)]
#[command(name = "opl", about = "Opolys wallet CLI", version)]
struct Cli {
    /// RPC server URL (default: https://localhost:4171)
    #[arg(long, default_value = DEFAULT_RPC_URL)]
    rpc_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug, Clone, Copy)]
struct MnemonicInput {
    /// Read the mnemonic from OPOLYS_MNEMONIC
    #[arg(long, conflicts_with = "from_stdin")]
    from_env: bool,

    /// Prompt for the mnemonic without echoing it
    #[arg(long, conflicts_with = "from_env")]
    from_stdin: bool,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Generate a new 24-word mnemonic and show the derived address
    New {
        /// Account index to derive (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },

    /// Show the address for a given mnemonic
    Address {
        #[command(flatten)]
        mnemonic: MnemonicInput,

        /// Account index to derive (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },

    /// Check account balance via RPC
    Balance {
        /// Account address (hex ObjectId)
        address: String,
    },

    /// Create a signed transfer transaction
    Transfer {
        #[command(flatten)]
        mnemonic: MnemonicInput,

        /// Recipient address (hex ObjectId)
        recipient: String,

        /// Amount in OPL (e.g. "10.5")
        amount: String,

        /// Fee in OPL (default: 0.000001 = 1 Flake)
        #[arg(long, default_value = "0.000001")]
        fee: String,

        /// Nonce for this account (query from RPC if not provided)
        #[arg(long)]
        nonce: Option<u64>,

        /// Account index (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },

    /// Create a signed refiner bond transaction
    Bond {
        #[command(flatten)]
        mnemonic: MnemonicInput,

        /// Amount to bond in OPL
        amount: String,

        /// Fee in OPL (default: 0.000001)
        #[arg(long, default_value = "0.000001")]
        fee: String,

        /// Nonce for this account
        #[arg(long)]
        nonce: Option<u64>,

        /// Account index (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },

    /// Create a signed refiner unbond transaction
    Unbond {
        #[command(flatten)]
        mnemonic: MnemonicInput,

        /// Amount to unbond in OPL
        amount: String,

        /// Fee in OPL (default: 0.000001)
        #[arg(long, default_value = "0.000001")]
        fee: String,

        /// Nonce for this account
        #[arg(long)]
        nonce: Option<u64>,

        /// Account index (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },

    /// Broadcast a signed transaction to the network via RPC
    Send {
        /// Hex-encoded signed transaction (Borsh-serialized)
        tx_hex: String,
    },
}

fn parse_opl_amount(s: &str) -> Result<FlakeAmount, String> {
    let parts: Vec<&str> = s.split('.').collect();
    match parts.len() {
        1 => {
            let whole: u64 = parts[0]
                .parse()
                .map_err(|e| format!("Invalid amount: {}", e))?;
            Ok(whole * FLAKES_PER_OPL)
        }
        2 => {
            let whole: u64 = parts[0]
                .parse()
                .map_err(|e| format!("Invalid whole amount: {}", e))?;
            let frac_str = parts[1];
            if frac_str.len() > 6 {
                return Err("Too many decimal places (max 6)".to_string());
            }
            let frac_str_padded = format!("{:0<6}", frac_str);
            let frac: u64 = frac_str_padded[..6]
                .parse()
                .map_err(|e| format!("Invalid fraction: {}", e))?;
            Ok(whole * FLAKES_PER_OPL + frac)
        }
        _ => Err("Invalid amount format".to_string()),
    }
}

fn read_mnemonic(input: MnemonicInput) -> Result<Bip39Mnemonic, Box<dyn std::error::Error>> {
    let phrase = if input.from_env {
        std::env::var("OPOLYS_MNEMONIC").map_err(|_| "OPOLYS_MNEMONIC is not set")?
    } else if input.from_stdin {
        rpassword::prompt_password("Mnemonic: ")?
    } else {
        return Err("Choose --from-env or --from-stdin to provide the mnemonic".into());
    };

    Ok(Bip39Mnemonic::from_words(phrase.trim())?)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let chain_id = MAINNET_CHAIN_ID;
    warn_if_insecure_rpc_url(&cli.rpc_url);
    match cli.command {
        Command::New { account } => {
            let mnemonic = Bip39Mnemonic::generate()?;
            let phrase = mnemonic.phrase();
            eprintln!("Mnemonic (24 words):");
            eprintln!("{}", phrase);
            eprintln!();

            let seed = mnemonic.to_seed("");
            let keypair = seed.derive_keypair(account);
            println!(
                "Address (account {}): {}",
                account,
                keypair.object_id().to_hex()
            );
            eprintln!();
            eprintln!("IMPORTANT: Write down this mnemonic and keep it safe.");
            eprintln!("Anyone with this phrase can access your funds.");
        }

        Command::Address { mnemonic, account } => {
            let mnemonic = read_mnemonic(mnemonic)?;
            let seed = mnemonic.to_seed("");
            let keypair = seed.derive_keypair(account);
            println!("{}", keypair.object_id().to_hex());
        }

        Command::Balance { address } => {
            let client = reqwest::Client::new();
            let resp = client
                .post(&format!("{}/rpc", cli.rpc_url))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "opl_getBalance",
                    "params": [address],
                    "id": 1
                }))
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;
            if let Some(error) = body.get("error") {
                return Err(format!("RPC error: {}", error).into());
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&body.get("result").unwrap_or(&body))?
            );
        }

        Command::Transfer {
            mnemonic,
            recipient,
            amount,
            fee,
            nonce,
            account,
        } => {
            let mnemonic = read_mnemonic(mnemonic)?;
            let seed = mnemonic.to_seed("");
            let keypair = seed.derive_keypair(account);

            let recipient_id = opolys_core::ObjectId::from_hex(&recipient)
                .map_err(|e| format!("Invalid recipient address: {}", e))?;
            let amount_flakes = parse_opl_amount(&amount)?;
            let fee_flakes = parse_opl_amount(&fee)?;

            let nonce_val = match nonce {
                Some(n) => n,
                None => query_nonce(&cli.rpc_url, keypair.object_id().to_hex()).await?,
            };

            let tx = TransactionSigner::create_transfer(
                &keypair,
                recipient_id,
                amount_flakes,
                fee_flakes,
                nonce_val,
                chain_id,
            );

            let tx_bytes = borsh::to_vec(&tx)?;
            println!("{}", hex::encode(tx_bytes));
        }

        Command::Bond {
            mnemonic,
            amount,
            fee,
            nonce,
            account,
        } => {
            let mnemonic = read_mnemonic(mnemonic)?;
            let seed = mnemonic.to_seed("");
            let keypair = seed.derive_keypair(account);

            let amount_flakes = parse_opl_amount(&amount)?;
            let fee_flakes = parse_opl_amount(&fee)?;

            let nonce_val = match nonce {
                Some(n) => n,
                None => query_nonce(&cli.rpc_url, keypair.object_id().to_hex()).await?,
            };

            let tx = TransactionSigner::create_refiner_bond(
                &keypair,
                amount_flakes,
                fee_flakes,
                nonce_val,
                chain_id,
            );

            let tx_bytes = borsh::to_vec(&tx)?;
            println!("{}", hex::encode(tx_bytes));
        }

        Command::Unbond {
            mnemonic,
            amount,
            fee,
            nonce,
            account,
        } => {
            let mnemonic = read_mnemonic(mnemonic)?;
            let seed = mnemonic.to_seed("");
            let keypair = seed.derive_keypair(account);

            let amount_flakes = parse_opl_amount(&amount)?;
            let fee_flakes = parse_opl_amount(&fee)?;

            let nonce_val = match nonce {
                Some(n) => n,
                None => query_nonce(&cli.rpc_url, keypair.object_id().to_hex()).await?,
            };

            let tx = TransactionSigner::create_refiner_unbond(
                &keypair,
                amount_flakes,
                fee_flakes,
                nonce_val,
                chain_id,
            );

            let tx_bytes = borsh::to_vec(&tx)?;
            println!("{}", hex::encode(tx_bytes));
        }

        Command::Send { tx_hex } => {
            let client = reqwest::Client::new();
            let resp = client
                .post(&format!("{}/rpc", cli.rpc_url))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "opl_sendTransaction",
                    "params": [tx_hex],
                    "id": 1
                }))
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;
            if let Some(error) = body.get("error") {
                return Err(format!("RPC error: {}", error).into());
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&body.get("result").unwrap_or(&body))?
            );
        }
    }

    Ok(())
}

fn is_insecure_rpc_url(rpc_url: &str) -> bool {
    rpc_url
        .get(..7)
        .map(|prefix| prefix.eq_ignore_ascii_case("http://"))
        .unwrap_or(false)
}

fn warn_if_insecure_rpc_url(rpc_url: &str) {
    if is_insecure_rpc_url(rpc_url) {
        eprintln!("Warning: RPC URL uses http://. Use https:// for mainnet wallet operations.");
    }
}

async fn query_nonce(rpc_url: &str, address: String) -> Result<u64, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let resp = client
        .post(&format!("{}/rpc", rpc_url))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "opl_getAccount",
            "params": [address],
            "id": 1
        }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    let result = body.get("result").ok_or("No result in RPC response")?;
    let nonce = result
        .get("nonce")
        .and_then(|n| n.as_u64())
        .ok_or("No nonce in account response")?;
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_defaults_to_https_rpc_url() {
        let cli = Cli::parse_from(["opl", "new"]);

        assert_eq!(cli.rpc_url, DEFAULT_RPC_URL);
    }

    #[test]
    fn insecure_rpc_detection_is_scheme_based() {
        assert!(is_insecure_rpc_url("http://localhost:4171"));
        assert!(is_insecure_rpc_url("HTTP://localhost:4171"));
        assert!(!is_insecure_rpc_url("https://localhost:4171"));
        assert!(!is_insecure_rpc_url("httpx://localhost:4171"));
    }
}
