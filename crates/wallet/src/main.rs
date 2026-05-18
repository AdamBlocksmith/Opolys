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
use reqwest::Url;
use std::path::PathBuf;

const DEFAULT_RPC_URL: &str = "http://127.0.0.1:4171";

#[derive(Parser, Debug)]
#[command(name = "opl", about = "Opolys wallet CLI", version)]
struct Cli {
    /// RPC server URL (default: http://127.0.0.1:4171)
    #[arg(long, default_value = DEFAULT_RPC_URL)]
    rpc_url: String,

    /// API key for authenticated write RPC methods.
    ///
    /// The node requires this for opl_sendTransaction unless it was started
    /// with --no-rpc-auth. Read-only wallet commands do not need it.
    #[arg(long)]
    rpc_api_key: Option<String>,

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

    /// Write the derived account seed to a node-compatible key file
    ExportKeyFile {
        #[command(flatten)]
        mnemonic: MnemonicInput,

        /// Output path for the 32-byte ed25519 seed file
        output: PathBuf,

        /// Account index (default: 0)
        #[arg(long, default_value = "0")]
        account: u32,
    },

    /// Check account balance via RPC
    Balance {
        /// Account address (hex ObjectId)
        address: String,
    },

    /// Show the live dynamic minimum refiner bond via RPC
    BondMinimum,

    /// Show refiner status and recent hallmark activity via RPC
    Refiner {
        /// Refiner ObjectId/address hex
        address: String,

        /// Optional lookback blocks for recent hallmark scan
        #[arg(long)]
        lookback: Option<u64>,
    },

    /// Show a block assay certificate via RPC
    Assay {
        /// Block height or 32-byte block hash hex
        block: String,
    },

    /// Create a signed transfer transaction
    Transfer {
        #[command(flatten)]
        mnemonic: MnemonicInput,

        /// Recipient address (hex ObjectId)
        recipient: String,

        /// Amount in OPL (e.g. "10.5")
        amount: String,

        /// Fee in OPL. If omitted, uses the chain's current suggested fee.
        #[arg(long)]
        fee: Option<String>,

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

        /// Fee in OPL. If omitted, uses the chain's current suggested fee.
        #[arg(long)]
        fee: Option<String>,

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

        /// Fee in OPL. If omitted, uses the chain's current suggested fee.
        #[arg(long)]
        fee: Option<String>,

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
            whole
                .checked_mul(FLAKES_PER_OPL)
                .ok_or_else(|| "Amount is too large".to_string())
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
            whole
                .checked_mul(FLAKES_PER_OPL)
                .and_then(|base| base.checked_add(frac))
                .ok_or_else(|| "Amount is too large".to_string())
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
    validate_rpc_url(&cli.rpc_url)?;
    match cli.command {
        Command::New { account } => {
            let mnemonic = Bip39Mnemonic::generate()?;
            let phrase = mnemonic.phrase();
            eprintln!("Mnemonic (24 words):");
            eprintln!("{}", phrase.as_str());
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

        Command::ExportKeyFile {
            mnemonic,
            output,
            account,
        } => {
            let mnemonic = read_mnemonic(mnemonic)?;
            let seed = mnemonic.to_seed("");
            let keypair = seed.derive_keypair(account);
            keypair.write_key_file(&output)?;
            println!("{}", keypair.object_id().to_hex());
        }

        Command::Balance { address } => {
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("{}/rpc", cli.rpc_url))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "opl_getBalance",
                    "params": [address],
                    "id": 1
                }))
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;
            if let Some(error) = rpc_error(&body) {
                return Err(format!("RPC error: {}", error).into());
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&body.get("result").unwrap_or(&body))?
            );
        }

        Command::BondMinimum => {
            let minimum = query_minimum_refiner_bond(&cli.rpc_url).await?;
            let suggested = minimum
                .checked_add(10 * FLAKES_PER_OPL)
                .ok_or("Suggested bond amount overflowed")?;

            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "minimum_refiner_bond_flakes": minimum,
                    "minimum_refiner_bond_opl": format_flakes(minimum),
                    "suggested_buffered_bond_flakes": suggested,
                    "suggested_buffered_bond_opl": format_flakes(suggested),
                    "note": "Bonding exactly the current minimum can fail if new blocks raise total issued before inclusion."
                }))?
            );
        }

        Command::Refiner { address, lookback } => {
            let status = query_refiner_hallmark(&cli.rpc_url, &address, lookback).await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }

        Command::Assay { block } => {
            let certificate = query_block_assay_certificate(&cli.rpc_url, &block).await?;
            println!("{}", serde_json::to_string_pretty(&certificate)?);
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
            let fee_flakes = resolve_fee(&cli.rpc_url, fee).await?;

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
            )?;

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
            let fee_flakes = resolve_fee(&cli.rpc_url, fee).await?;

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
            )?;

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
            let fee_flakes = resolve_fee(&cli.rpc_url, fee).await?;

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
            )?;

            let tx_bytes = borsh::to_vec(&tx)?;
            println!("{}", hex::encode(tx_bytes));
        }

        Command::Send { tx_hex } => {
            let client = reqwest::Client::new();
            let request = client
                .post(format!("{}/rpc", cli.rpc_url))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "opl_sendTransaction",
                    "params": [tx_hex],
                    "id": 1
                }));
            let resp = with_rpc_api_key(request, cli.rpc_api_key.as_deref())
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;
            if let Some(error) = rpc_error(&body) {
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

fn with_rpc_api_key(
    request: reqwest::RequestBuilder,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    match api_key {
        Some(key) => request.header("X-Api-Key", key),
        None => request,
    }
}

fn rpc_error(body: &serde_json::Value) -> Option<&serde_json::Value> {
    body.get("error").filter(|error| !error.is_null())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
}

fn validate_rpc_url(rpc_url: &str) -> Result<(), String> {
    let url = Url::parse(rpc_url).map_err(|e| format!("Invalid RPC URL: {}", e))?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url.host_str().ok_or("RPC URL is missing a host")?;
            if is_loopback_host(host) {
                Ok(())
            } else {
                Err(format!(
                    "Refusing non-loopback http:// RPC URL '{}'. Use https:// for remote RPC endpoints.",
                    rpc_url
                ))
            }
        }
        scheme => Err(format!(
            "Unsupported RPC URL scheme '{}'. Use http:// for local loopback or https:// for remote RPC.",
            scheme
        )),
    }
}

async fn query_nonce(rpc_url: &str, address: String) -> Result<u64, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/rpc", rpc_url))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "opl_getAccount",
            "params": [address],
            "id": 1
        }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    if let Some(error) = rpc_error(&body) {
        return Err(format!("RPC error while querying account nonce: {}", error).into());
    }
    let result = body.get("result").ok_or("No result in RPC response")?;
    let nonce = result
        .get("nonce")
        .and_then(|n| n.as_u64())
        .ok_or("No nonce in account response")?;
    Ok(nonce)
}

async fn query_suggested_fee(rpc_url: &str) -> Result<FlakeAmount, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/rpc", rpc_url))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "opl_getChainInfo",
            "params": [],
            "id": 1
        }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    if let Some(error) = rpc_error(&body) {
        return Err(format!("RPC error while querying suggested fee: {}", error).into());
    }
    let result = body.get("result").ok_or("No result in RPC response")?;
    let suggested_fee = result
        .get("suggested_fee")
        .and_then(|n| n.as_u64())
        .ok_or("No suggested_fee in chain info response")?;
    Ok(suggested_fee.max(opolys_core::MIN_FEE))
}

async fn query_minimum_refiner_bond(
    rpc_url: &str,
) -> Result<FlakeAmount, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/rpc", rpc_url))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "opl_getChainInfo",
            "params": [],
            "id": 1
        }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    if let Some(error) = rpc_error(&body) {
        return Err(format!("RPC error while querying refiner bond minimum: {}", error).into());
    }
    let result = body.get("result").ok_or("No result in RPC response")?;
    let minimum = result
        .get("minimum_refiner_bond_flakes")
        .and_then(|n| n.as_u64())
        .ok_or("No minimum_refiner_bond_flakes in chain info response")?;
    Ok(minimum)
}

async fn query_refiner_hallmark(
    rpc_url: &str,
    address: &str,
    lookback: Option<u64>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let params = match lookback {
        Some(lookback) => serde_json::json!([address, lookback]),
        None => serde_json::json!([address]),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/rpc", rpc_url))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "opl_getRefinerHallmark",
            "params": params,
            "id": 1
        }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    if let Some(error) = rpc_error(&body) {
        return Err(format!("RPC error while querying refiner hallmark: {}", error).into());
    }
    Ok(body.get("result").cloned().unwrap_or(body))
}

async fn query_block_assay_certificate(
    rpc_url: &str,
    block: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let selector = match block.parse::<u64>() {
        Ok(height) => serde_json::json!(height),
        Err(_) => serde_json::json!(block),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/rpc", rpc_url))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "opl_getBlockAssayCertificate",
            "params": [selector],
            "id": 1
        }))
        .send()
        .await?;

    let body: serde_json::Value = resp.json().await?;
    if let Some(error) = rpc_error(&body) {
        return Err(format!(
            "RPC error while querying block assay certificate: {}",
            error
        )
        .into());
    }
    Ok(body.get("result").cloned().unwrap_or(body))
}

fn format_flakes(flakes: FlakeAmount) -> String {
    format!(
        "{}.{:06} OPL",
        flakes / FLAKES_PER_OPL,
        flakes % FLAKES_PER_OPL
    )
}

async fn resolve_fee(
    rpc_url: &str,
    explicit_fee: Option<String>,
) -> Result<FlakeAmount, Box<dyn std::error::Error>> {
    match explicit_fee {
        Some(fee) => Ok(parse_opl_amount(&fee)?),
        None => query_suggested_fee(rpc_url).await.map_err(|e| {
            format!(
                "Could not query chain suggested fee from RPC: {}. Pass --fee explicitly for offline signing.",
                e
            )
            .into()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_opl_amount_rejects_overflow() {
        assert!(parse_opl_amount("18446744073710").is_err());
        assert!(parse_opl_amount("18446744073709.551616").is_err());
    }

    #[test]
    fn parse_opl_amount_accepts_fractional_flakes() {
        assert_eq!(parse_opl_amount("1").unwrap(), FLAKES_PER_OPL);
        assert_eq!(parse_opl_amount("1.000001").unwrap(), FLAKES_PER_OPL + 1);
    }

    #[test]
    fn cli_defaults_to_loopback_rpc_url() {
        let cli = Cli::parse_from(["opl", "new"]);

        assert_eq!(cli.rpc_url, DEFAULT_RPC_URL);
    }

    #[test]
    fn cli_accepts_rpc_api_key_for_write_calls() {
        let cli = Cli::parse_from(["opl", "--rpc-api-key", "secret", "send", "00"]);

        assert_eq!(cli.rpc_api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn export_key_file_accepts_output_path() {
        let cli = Cli::parse_from(["opl", "export-key-file", "--from-env", "miner.key"]);

        let Command::ExportKeyFile {
            output, account, ..
        } = cli.command
        else {
            panic!("expected export-key-file command");
        };
        assert_eq!(output, std::path::PathBuf::from("miner.key"));
        assert_eq!(account, 0);
    }

    #[test]
    fn transfer_fee_defaults_to_chain_suggestion() {
        let cli = Cli::parse_from([
            "opl",
            "transfer",
            "--from-env",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1",
        ]);

        let Command::Transfer { fee, .. } = cli.command else {
            panic!("expected transfer command");
        };
        assert!(fee.is_none());
    }

    #[test]
    fn transfer_fee_allows_explicit_override() {
        let cli = Cli::parse_from([
            "opl",
            "transfer",
            "--from-env",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1",
            "--fee",
            "0.123456",
        ]);

        let Command::Transfer { fee, .. } = cli.command else {
            panic!("expected transfer command");
        };
        assert_eq!(fee.as_deref(), Some("0.123456"));
    }

    #[test]
    fn bond_minimum_command_parses() {
        let cli = Cli::parse_from(["opl", "bond-minimum"]);

        assert!(matches!(cli.command, Command::BondMinimum));
    }

    #[test]
    fn refiner_command_parses_with_optional_lookback() {
        let cli = Cli::parse_from(["opl", "refiner", "aa", "--lookback", "12"]);

        let Command::Refiner { address, lookback } = cli.command else {
            panic!("expected refiner command");
        };
        assert_eq!(address, "aa");
        assert_eq!(lookback, Some(12));
    }

    #[test]
    fn assay_command_parses_height_or_hash() {
        let cli = Cli::parse_from(["opl", "assay", "12"]);

        let Command::Assay { block } = cli.command else {
            panic!("expected assay command");
        };
        assert_eq!(block, "12");
    }

    #[test]
    fn format_flakes_uses_six_decimal_places() {
        assert_eq!(format_flakes(1), "0.000001 OPL");
        assert_eq!(format_flakes(21 * FLAKES_PER_OPL), "21.000000 OPL");
    }

    #[test]
    fn rpc_url_validation_allows_loopback_http_and_https() {
        assert!(validate_rpc_url("http://localhost:4171").is_ok());
        assert!(validate_rpc_url("http://127.0.0.1:4171").is_ok());
        assert!(validate_rpc_url("http://[::1]:4171").is_ok());
        assert!(validate_rpc_url("https://rpc.opolys.example").is_ok());
    }

    #[test]
    fn rpc_url_validation_rejects_remote_http() {
        let err = validate_rpc_url("http://192.0.2.10:4171").unwrap_err();

        assert!(err.contains("Refusing non-loopback http:// RPC URL"));
    }

    #[test]
    fn rpc_url_validation_rejects_unsupported_schemes() {
        let err = validate_rpc_url("ws://localhost:4171").unwrap_err();

        assert!(err.contains("Unsupported RPC URL scheme"));
    }

    #[test]
    fn rpc_error_ignores_null_error_field() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "result": true,
            "error": null
        });

        assert!(rpc_error(&body).is_none());
    }

    #[test]
    fn rpc_error_detects_error_object() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32000, "message": "nope" }
        });

        assert!(rpc_error(&body).is_some());
    }
}
