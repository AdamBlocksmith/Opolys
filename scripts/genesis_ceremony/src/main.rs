//! Genesis Ceremony — Opolys ($OPL) chain initialization tool.
//!
//! Fetches live gold spot-price data and the most recent annual mine-production
//! figures from multiple independent sources, applies a trimmed-median / outlier
//! algorithm, computes the canonical BASE_REWARD, and produces a cryptographically
//! attested, operator-signed output set that anyone can independently verify.
//!
//! DATA PHILOSOPHY
//! ───────────────
//! • Gold spot price          — live, real-time at ceremony moment
//! • Annual mine production   — most recent published figure (USGS/WGC, annual)
//!   Gold production is measured and published once per year. Using the most
//!   recent published figure is correct and honest; it is not "historical" data.
//!
//! CEREMONY WINDOW
//! ───────────────
//! The entire ceremony must complete within 5 minutes. If the window is exceeded
//! the binary aborts. All source fetches have a 30-second individual timeout.
//! A warning is emitted if any two price-source fetches are more than 60 seconds
//! apart (market price may have moved between those reads).
//!
//! MODES
//! ─────
//!   (default)     Fetch all sources concurrently; fall back to manual prompt
//!                 for any source that cannot be parsed automatically.
//!   --manual      Skip all network fetches; prompt the operator for every value.
//!   --dry-run     Use hard-coded 2024 USGS/WGC/LBMA actuals. No network I/O,
//!                 no operator prompts, uses a deterministic test signing key.
//!
//! OUTPUT FILES  (written to --output-dir, default ".")
//! ─────────────────────────────────────────────────────
//!   genesis_attestation.json   Full attested record with per-source timestamps
//!                              and Blake3 response hashes.
//!   genesis_params.rs          Ready-to-paste Rust constants.
//!   genesis_verification.txt   Step-by-step independent verification guide.
//!   operator_signing_key.txt   Operator ed25519 seed (KEEP SECRET). Written
//!                              only when --operator-key-file is not supplied.

use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

// ─── Physical constants ───────────────────────────────────────────────────────

const TROY_OZ_PER_TONNE: f64 = 32_150.7;
const BLOCK_TARGET_TIME_MS: u64 = 84_375;
const FLAKES_PER_OPL: u64 = 1_000_000;

// Sanity bounds — values outside these are rejected before the algorithm runs
const PROD_MIN_TONNES: f64 = 1_000.0;
const PROD_MAX_TONNES: f64 = 10_000.0;
const PRICE_MIN_USD_OZ: f64 = 500.0;
const PRICE_MAX_USD_OZ: f64 = 15_000.0;

// Algorithm parameters
const OUTLIER_PCT: f64 = 0.15;  // flag if >15% from median
const MIN_PROD_SOURCES: usize = 5; // abort if fewer production sources succeed

// Timing constraints
const FETCH_TIMEOUT_SECS: u64 = 30;
const CEREMONY_WINDOW_SECS: u64 = 300; // 5-minute hard limit
const PRICE_SPREAD_WARN_MS: u64 = 60_000; // warn if price fetches >60s apart

// Dry-run hard-coded actuals (USGS/WGC 2024, LBMA 2024 annual average)
const DRY_RUN_PROD_TONNES: f64 = 3_630.0;
const DRY_RUN_PRICE_USD_OZ: f64 = 2_386.0;
const DRY_RUN_PROD_YEAR: u32 = 2024;
// Deterministic test-only signing key for --dry-run (NOT for real ceremonies)
const DRY_RUN_KEY_SEED: [u8; 32] = [42u8; 32];

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "genesis-ceremony",
    about = "Opolys genesis ceremony — compute BASE_REWARD from live gold data",
    long_about = None
)]
struct Cli {
    /// Use hard-coded 2024 actuals. No network calls, no prompts, test signing key.
    #[arg(long)]
    dry_run: bool,

    /// Prompt the operator for every value manually. Skip all network fetches.
    #[arg(long)]
    manual: bool,

    /// Name of the ceremony operator (recorded in attestation).
    #[arg(long, default_value = "unknown")]
    operator: String,

    /// Which calendar year's production data is being recorded.
    /// USGS/WGC figures are annual and published with a ~1 year lag.
    /// Defaults to the previous calendar year.
    #[arg(long)]
    production_year: Option<u32>,

    /// Path to the operator's ed25519 signing key file (64-char hex seed).
    /// If not provided, a new key is generated and saved to --output-dir.
    #[arg(long)]
    operator_key_file: Option<PathBuf>,

    /// Directory to write output files.
    #[arg(long, default_value = ".")]
    output_dir: PathBuf,
}

// ─── Source definitions ───────────────────────────────────────────────────────

struct SourceDef {
    name: &'static str,
    url: &'static str,
    /// Instructions shown when automatic extraction fails.
    instructions: &'static str,
}

static PROD_SOURCES: &[SourceDef] = &[
    SourceDef {
        name: "USGS",
        url: "https://minerals.usgs.gov/minerals/pubs/commodity/gold/",
        instructions: "Find the 'Mine production, world' row in the most recent Mineral \
                       Commodity Summaries table. Enter world annual production in metric tonnes.",
    },
    SourceDef {
        name: "World Gold Council",
        url: "https://www.gold.org/goldhub/data/gold-demand-statistics",
        instructions: "On the WGC Gold Demand Statistics page, find 'Mine production' \
                       under Supply. Enter the most recent full-year figure in metric tonnes.",
    },
    SourceDef {
        name: "Kitco Production",
        url: "https://www.kitco.com/charts/goldproduction.html",
        instructions: "On Kitco's production page, find the most recent annual world \
                       mine production figure. Enter in metric tonnes.",
    },
    SourceDef {
        name: "MacroTrends",
        url: "https://www.macrotrends.net/1369/gold-price-history",
        instructions: "MacroTrends includes annual production data. Find the most recent \
                       year's world mine output. Enter in metric tonnes.",
    },
    SourceDef {
        name: "Trading Economics",
        url: "https://tradingeconomics.com/commodity/gold",
        instructions: "Check the Trading Economics gold production section. Enter the most \
                       recent annual world mine output in metric tonnes.",
    },
    SourceDef {
        name: "LBMA Annual Survey",
        url: "https://www.lbma.org.uk/prices-and-data/precious-metal-prices",
        instructions: "The LBMA publishes annual gold statistics. Find 'World mine production' \
                       in their annual alchemist or survey. Enter in metric tonnes.",
    },
    SourceDef {
        name: "Metals Focus",
        url: "https://metalsfocus.com",
        instructions: "Metals Focus publishes annual gold mine supply data (may require \
                       subscription or press release). Enter world mine production for the \
                       most recent full year in metric tonnes.",
    },
];

static PRICE_SOURCES: &[SourceDef] = &[
    SourceDef {
        name: "CME Group COMEX",
        url: "https://www.cmegroup.com/markets/metals/precious/gold.html",
        instructions: "On the CME Group COMEX gold page, find the current front-month \
                       settlement or live spot price. Enter in USD per troy ounce.",
    },
    SourceDef {
        name: "LBMA Live Price",
        url: "https://www.lbma.org.uk/prices-and-data/precious-metal-prices",
        instructions: "The LBMA publishes live gold price data. Enter the most recent \
                       PM fix or live price in USD per troy ounce.",
    },
    SourceDef {
        name: "Kitco Spot",
        url: "https://www.kitco.com/gold-price-today-usa/",
        instructions: "On Kitco's live gold price page, find the current spot price \
                       (bid or ask). Enter in USD per troy ounce.",
    },
    SourceDef {
        name: "BullionVault",
        url: "https://api.bullionvault.com/gold-price-chart.json",
        instructions: "BullionVault's API endpoint returns live gold price data. \
                       Enter the current USD price per troy ounce.",
    },
    SourceDef {
        name: "Goldprice.org",
        url: "https://goldprice.org/gold-price.html",
        instructions: "On goldprice.org, find the current live spot gold price. \
                       Enter in USD per troy ounce.",
    },
    SourceDef {
        name: "Reuters/LSEG",
        url: "https://www.lseg.com/en/financial-data/financial-markets/commodities",
        instructions: "On the LSEG commodities page, find the current gold spot price. \
                       Enter in USD per troy ounce.",
    },
];

// ─── Data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceResult {
    name: String,
    url: String,
    /// Unix timestamp in milliseconds when the HTTP response was received.
    /// 0 means the fetch was not attempted (manual mode) or failed.
    fetched_at_ms: u64,
    /// Blake3-256 hex of the raw HTTP response body (or hash of b"" if failed).
    raw_response_hash: String,
    /// Extracted value: tonnes for production sources, USD/oz for price sources.
    extracted_value: Option<f64>,
    /// "ok" | "failed" | "outlier" | "manual" | "manual-outlier"
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenesisAttestation {
    /// Unix milliseconds when the ceremony started (first fetch dispatched).
    ceremony_start_ms: u64,
    /// Unix milliseconds when the operator confirmed and files were written.
    ceremony_end_ms: u64,
    /// Unix seconds at ceremony start (= ceremony_start_ms / 1000).
    ceremony_timestamp: u64,
    operator_name: String,
    /// ed25519 verifying key hex (safe to publish; stored in genesis block).
    operator_public_key: String,
    /// ed25519 signature over the master_hash bytes. Set after operator confirmation.
    /// Excluded from master_hash computation (set to "" when hashing).
    operator_signature: String,
    /// Calendar year of the production data (e.g. 2024).
    production_data_year: u32,
    production_sources: Vec<SourceResult>,
    price_sources: Vec<SourceResult>,
    /// Max spread in ms between the earliest and latest price-source fetch timestamps.
    /// 0 in manual or dry-run mode. Warning emitted if > 60,000 ms (60 seconds).
    price_fetch_spread_ms: u64,
    median_production_tonnes: f64,
    median_price_usd_cents: u64,
    blocks_per_year: u64,
    base_reward_opl: u64,
    base_reward_flakes: u64,
    derivation_steps: Vec<String>,
    /// Blake3-256 of the full attestation JSON with master_hash="" and operator_signature="".
    master_hash: String,
}

// ─── Fetch utilities ──────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn empty_hash() -> String {
    hex::encode(blake3::hash(b"").as_bytes())
}

/// Fetch a URL; returns (body, blake3_hex, fetched_at_ms).
/// On any error returns ("", hash_of_empty, 0).
async fn fetch(url: &str) -> (String, String, u64) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("Mozilla/5.0 (compatible; OplGenesisCeremony/1.0)")
        .build()
    {
        Ok(c) => c,
        Err(_) => return (String::new(), empty_hash(), 0),
    };

    match client.get(url).send().await {
        Ok(resp) => {
            let body = resp.text().await.unwrap_or_default();
            let hash = hex::encode(blake3::hash(body.as_bytes()).as_bytes());
            let fetched_at_ms = now_ms();
            (body, hash, fetched_at_ms)
        }
        Err(_) => (String::new(), empty_hash(), 0),
    }
}

/// Spawn one task per source, run all concurrently, collect (name, hash, value, fetched_at_ms).
async fn run_fetch_phase(
    sources: &[SourceDef],
    parse_fn: fn(&str, &str) -> Option<f64>,
) -> Vec<(String, String, Option<f64>, u64)> {
    let handles: Vec<_> = sources
        .iter()
        .map(|s| {
            let url = s.url.to_string();
            let name = s.name.to_string();
            tokio::spawn(async move {
                let (body, hash, fetched_at_ms) = fetch(&url).await;
                let value = parse_fn(&name, &body);
                (name, hash, value, fetched_at_ms)
            })
        })
        .collect();

    let mut results = Vec::new();
    for h in handles {
        results.push(
            h.await.unwrap_or_else(|_| ("error".into(), empty_hash(), None, 0u64)),
        );
    }
    results
}

// ─── Parse utilities ──────────────────────────────────────────────────────────

fn parse_production_tonnes(source_name: &str, body: &str) -> Option<f64> {
    if body.is_empty() || source_name == "BullionVault" {
        return None;
    }
    let text = body.to_lowercase();
    let keywords = ["tonne", "metric ton", "mine production", "world production"];
    for kw in &keywords {
        if let Some(pos) = text.find(kw) {
            let before = &body[pos.saturating_sub(200)..pos];
            if let Some(n) = last_number_in_range(before, PROD_MIN_TONNES, PROD_MAX_TONNES) {
                return Some(n);
            }
            let end = (pos + kw.len() + 200).min(body.len());
            let after = &body[pos + kw.len()..end];
            if let Some(n) = last_number_in_range(after, PROD_MIN_TONNES, PROD_MAX_TONNES) {
                return Some(n);
            }
        }
    }
    None
}

fn parse_price_usd_oz(source_name: &str, body: &str) -> Option<f64> {
    if body.is_empty() {
        return None;
    }
    if source_name == "BullionVault" {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
            return find_price_in_json(&val);
        }
        return None;
    }
    let text = body.to_lowercase();
    let keywords = ["spot price", "gold price", "ask price", "bid price", "xau", "per oz", "per ounce"];
    for kw in &keywords {
        if let Some(pos) = text.find(kw) {
            let start = pos.saturating_sub(100);
            let end = (pos + kw.len() + 100).min(body.len());
            let window = &body[start..end];
            if let Some(n) = last_number_in_range(window, PRICE_MIN_USD_OZ, PRICE_MAX_USD_OZ) {
                return Some(n);
            }
        }
    }
    None
}

fn find_price_in_json(val: &serde_json::Value) -> Option<f64> {
    match val {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f >= PRICE_MIN_USD_OZ && f <= PRICE_MAX_USD_OZ { Some(f) } else { None }
        }
        serde_json::Value::Object(map) => {
            for key in &["price", "ask", "bid", "spot", "usd", "rate"] {
                if let Some(v) = map.get(*key) {
                    if let Some(f) = find_price_in_json(v) { return Some(f); }
                }
            }
            for v in map.values() {
                if let Some(f) = find_price_in_json(v) { return Some(f); }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let Some(f) = find_price_in_json(v) { return Some(f); }
            }
            None
        }
        _ => None,
    }
}

fn last_number_in_range(s: &str, min: f64, max: f64) -> Option<f64> {
    let mut result: Option<f64> = None;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == ',' || chars[i] == '.') {
                i += 1;
            }
            let num_str: String = chars[start..i].iter().filter(|&&c| c != ',').collect();
            if let Ok(n) = num_str.parse::<f64>() {
                if n >= min && n <= max { result = Some(n); }
            }
        } else {
            i += 1;
        }
    }
    result
}

// ─── Median + outlier algorithm ───────────────────────────────────────────────

fn trimmed_median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if values.len() >= 3 {
        values.remove(0);
        values.remove(values.len() - 1);
    }
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn apply_outlier_flags(results: &mut [SourceResult], median: f64) {
    for r in results.iter_mut() {
        if let Some(v) = r.extracted_value {
            if (v - median).abs() / median > OUTLIER_PCT {
                r.status = if r.status == "manual" {
                    "manual-outlier".to_string()
                } else {
                    "outlier".to_string()
                };
            }
        }
    }
}

fn price_fetch_spread_ms(results: &[SourceResult]) -> u64 {
    let ts: Vec<u64> = results.iter()
        .filter(|r| r.fetched_at_ms > 0 && r.extracted_value.is_some())
        .map(|r| r.fetched_at_ms)
        .collect();
    if ts.len() < 2 { return 0; }
    ts.iter().max().unwrap() - ts.iter().min().unwrap()
}

// ─── Manual prompts ───────────────────────────────────────────────────────────

fn prompt(msg: &str) -> String {
    print!("{}", msg);
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line).ok();
    line.trim().to_string()
}

fn prompt_value(def: &SourceDef, value_desc: &str, unit: &str, example: &str) -> Option<f64> {
    println!();
    println!("  Source  : {}", def.name);
    println!("  URL     : {}", def.url);
    println!("  How     : {}", def.instructions);
    println!("  Looking for: {} in {}", value_desc, unit);
    println!("  Example : {}", example);
    loop {
        let s = prompt(&format!("  Enter {} (or 'skip'): ", unit));
        if s.eq_ignore_ascii_case("skip") { return None; }
        let clean: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
        match clean.parse::<f64>() {
            Ok(n) if n > 0.0 => return Some(n),
            _ => println!("  Invalid number. Try again or type 'skip'."),
        }
    }
}

// ─── Collect source results ───────────────────────────────────────────────────

fn collect_source_results(
    sources: &[SourceDef],
    fetch_results: Vec<(String, String, Option<f64>, u64)>,
    manual: bool,
    value_desc: &str,
    unit: &str,
    example: &str,
    sanity_min: f64,
    sanity_max: f64,
) -> Vec<SourceResult> {
    let mut out = Vec::new();

    for (def, (_, hash, auto_value, auto_fetched_at)) in sources.iter().zip(fetch_results) {
        let (value, fetched_at_ms, was_manual) = if manual {
            let v = prompt_value(def, value_desc, unit, example);
            (v, now_ms(), true)
        } else if auto_value.is_some() {
            (auto_value, auto_fetched_at, false)
        } else {
            println!("\n  [auto-parse failed for {}]", def.name);
            let v = prompt_value(def, value_desc, unit, example);
            (v, now_ms(), true)
        };

        let value = value.filter(|&v| v >= sanity_min && v <= sanity_max);
        let status = match &value {
            None => "failed",
            Some(_) if was_manual => "manual",
            Some(_) => "ok",
        }.to_string();

        out.push(SourceResult {
            name: def.name.to_string(),
            url: def.url.to_string(),
            fetched_at_ms,
            raw_response_hash: hash,
            extracted_value: value,
            status,
        });
    }

    out
}

// ─── Operator key management ──────────────────────────────────────────────────

fn load_or_generate_signing_key(key_file: Option<&Path>, output_dir: &Path) -> (SigningKey, String) {
    if let Some(path) = key_file {
        let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("ERROR: cannot read operator key file {}: {}", path.display(), e);
            std::process::exit(1);
        });
        let hex_str: String = content.lines()
            .filter(|l| !l.starts_with('#'))
            .flat_map(|l| l.chars())
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        let seed_bytes = hex::decode(&hex_str).unwrap_or_else(|_| {
            eprintln!("ERROR: operator key file contains invalid hex");
            std::process::exit(1);
        });
        let seed: [u8; 32] = seed_bytes.try_into().unwrap_or_else(|_| {
            eprintln!("ERROR: operator key must be exactly 32 bytes (64 hex chars)");
            std::process::exit(1);
        });
        let sk = SigningKey::from_bytes(&seed);
        let pk_hex = hex::encode(sk.verifying_key().as_bytes());
        println!("  Loaded operator key from {}", path.display());
        println!("  Public key: {}", pk_hex);
        (sk, pk_hex)
    } else {
        let seed: [u8; 32] = rand::random();
        let sk = SigningKey::from_bytes(&seed);
        let seed_hex = hex::encode(sk.as_bytes());
        let pk_hex = hex::encode(sk.verifying_key().as_bytes());
        std::fs::create_dir_all(output_dir).ok();
        let key_path = output_dir.join("operator_signing_key.txt");
        let content = format!(
            "# OPOLYS GENESIS CEREMONY OPERATOR SIGNING KEY\n\
             # KEEP THIS SECRET. BACK UP OFFLINE. NEVER SHARE THE SEED.\n\
             # Public key (safe to share): {}\n\
             {}\n",
            pk_hex, seed_hex,
        );
        std::fs::write(&key_path, &content).unwrap_or_else(|e| {
            eprintln!("ERROR: cannot write operator key file: {}", e);
            std::process::exit(1);
        });
        println!("  Generated new operator ed25519 key.");
        println!("  !! SAVE THIS KEY FILE: {}", key_path.display());
        println!("  !! Losing it means you cannot re-sign this ceremony.");
        println!("  Public key: {}", pk_hex);
        (sk, pk_hex)
    }
}

fn sign_master_hash(sk: &SigningKey, master_hash_hex: &str) -> String {
    let hash_bytes = hex::decode(master_hash_hex).expect("master_hash must be valid hex");
    hex::encode(sk.sign(&hash_bytes).to_bytes())
}

// ─── Operator confirmation ────────────────────────────────────────────────────

fn operator_confirm(a: &GenesisAttestation, is_dry_run: bool) -> bool {
    let prod_ok = a.production_sources.iter().filter(|s| s.extracted_value.is_some()).count();
    let prod_total = a.production_sources.len();
    let price_ok = a.price_sources.iter().filter(|s| s.extracted_value.is_some()).count();
    let price_total = a.price_sources.len();

    let failed_sources: Vec<&str> = a.production_sources.iter()
        .chain(a.price_sources.iter())
        .filter(|s| s.extracted_value.is_none())
        .map(|s| s.name.as_str())
        .collect();

    let duration_secs = (a.ceremony_end_ms.saturating_sub(a.ceremony_start_ms)) / 1000;

    println!();
    println!("╔══════════════════════════════════════════════════╗");
    if is_dry_run {
    println!("║       OPERATOR CONFIRMATION  [DRY RUN]           ║");
    } else {
    println!("║           OPERATOR CONFIRMATION                  ║");
    }
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("  Ceremony timestamp   : {}", format_timestamp(a.ceremony_timestamp));
    println!("  Ceremony duration    : {}s / {}s window", duration_secs, CEREMONY_WINDOW_SECS);
    println!("  Production sources   : {}/{} responded", prod_ok, prod_total);
    println!("  Price sources        : {}/{} responded", price_ok, price_total);
    if !failed_sources.is_empty() {
        println!("  Failed sources       : {}", failed_sources.join(", "));
    }
    if a.price_fetch_spread_ms > PRICE_SPREAD_WARN_MS {
        println!();
        println!("  !! WARNING: price sources fetched {:.0}s apart (threshold: {:.0}s)",
            a.price_fetch_spread_ms as f64 / 1_000.0,
            PRICE_SPREAD_WARN_MS as f64 / 1_000.0);
        println!("  !! Market price may have moved between source reads.");
        println!("  !! Consider aborting and re-running if spread is too large.");
    }
    println!();
    println!("  Production data year : {} (most recent USGS/WGC annual figure)", a.production_data_year);
    println!("  Median production    : {:.1} t/yr", a.median_production_tonnes);
    println!("  Gold spot price      : ${:.2}/oz  [LIVE at ceremony time]",
        a.median_price_usd_cents as f64 / 100.0);
    println!("  Blocks per year      : {}", a.blocks_per_year);
    println!("  Computed BASE_REWARD : {} OPL per block", a.base_reward_opl);
    println!("  Master hash          : {}", a.master_hash);
    println!("  Operator public key  : {}", a.operator_public_key);
    if is_dry_run {
        println!();
        println!("  [DRY RUN] Signing key is test-only (seed=[42;32]). Not for production.");
    }
    println!();

    if is_dry_run {
        // Auto-confirm in dry-run mode
        println!("  [DRY RUN] Auto-confirming.");
        return true;
    }

    loop {
        let s = prompt("  Proceed and sign? (yes/no): ");
        match s.to_lowercase().as_str() {
            "yes" | "y" => return true,
            "no" | "n" => return false,
            _ => println!("  Please type 'yes' or 'no'."),
        }
    }
}

// ─── Computation helpers ──────────────────────────────────────────────────────

fn compute_blocks_per_year() -> u64 {
    let ms_per_year = 365.25_f64 * 86_400.0 * 1_000.0;
    (ms_per_year / BLOCK_TARGET_TIME_MS as f64).floor() as u64
}

fn compute_master_hash(attestation: &GenesisAttestation) -> String {
    let mut tmp = serde_json::to_value(attestation).unwrap();
    tmp["master_hash"] = serde_json::Value::String(String::new());
    tmp["operator_signature"] = serde_json::Value::String(String::new());
    let canonical = serde_json::to_string(&tmp).unwrap();
    hex::encode(blake3::hash(canonical.as_bytes()).as_bytes())
}

fn build_derivation_steps(
    median_tonnes: f64,
    blocks_per_year: u64,
    annual_oz: f64,
    base_reward_opl: u64,
    base_reward_flakes: u64,
) -> Vec<String> {
    vec![
        format!("median_production_tonnes = {:.4} t/yr (trimmed median, outliers flagged)", median_tonnes),
        format!("troy_oz_per_tonne        = {}", TROY_OZ_PER_TONNE),
        format!("annual_oz                = {:.4} × {:.1} = {:.4}", median_tonnes, TROY_OZ_PER_TONNE, annual_oz),
        format!("block_target_time_ms     = {}", BLOCK_TARGET_TIME_MS),
        format!("blocks_per_year          = floor(365.25 × 86400 × 1000 / {}) = {}", BLOCK_TARGET_TIME_MS, blocks_per_year),
        format!("base_reward_opl          = floor({:.4} / {}) = {}", annual_oz, blocks_per_year, base_reward_opl),
        format!("base_reward_flakes       = {} × {} = {}", base_reward_opl, FLAKES_PER_OPL, base_reward_flakes),
    ]
}

// ─── Output formatting ────────────────────────────────────────────────────────

fn format_timestamp(ts_secs: u64) -> String {
    let days = ts_secs / 86_400;
    let tod  = ts_secs % 86_400;
    let hh = tod / 3_600;
    let mm = (tod % 3_600) / 60;
    let ss = tod % 60;

    let mut y = 1970u64;
    let mut d = days;
    loop {
        let dy = if is_leap(y) { 366 } else { 365 };
        if d < dy { break; }
        d -= dy; y += 1;
    }
    let mo_days = if is_leap(y) {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 1u64;
    for &dm in &mo_days {
        if d < dm { break; }
        d -= dm; mo += 1;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, mo, d + 1, hh, mm, ss)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn format_flakes_constant(flakes: u64) -> String {
    let s = flakes.to_string();
    let mut r = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 { r.push('_'); }
        r.push(c);
    }
    r.chars().rev().collect()
}

// ─── Output writers ───────────────────────────────────────────────────────────

fn write_attestation_json(dir: &Path, a: &GenesisAttestation) -> std::io::Result<()> {
    let path = dir.join("genesis_attestation.json");
    std::fs::write(&path, serde_json::to_string_pretty(a).unwrap())?;
    println!("  Written: {}", path.display());
    Ok(())
}

fn write_params_rs(dir: &Path, a: &GenesisAttestation) -> std::io::Result<()> {
    let content = format!(
        "// Auto-generated by genesis-ceremony. Do not edit by hand.\n\
         // Ceremony : {dt}\n\
         // Operator : {op}\n\
         // Prod year: {year} (most recent USGS/WGC annual figure)\n\
         // Master hash: {mh}\n\
         //\n\
         // Paste these into crates/core/src/constants.rs and regenerate if needed.\n\n\
         pub const BASE_REWARD: u64 = {fr}; // {opl} OPL derived from ceremony\n\
         pub const CEREMONY_TIMESTAMP: u64 = {ts};\n\
         pub const CEREMONY_MASTER_HASH: &str = \"{mh}\";\n\
         pub const OPERATOR_PUBLIC_KEY: &str = \"{pk}\";\n\
         pub const OPERATOR_SIGNATURE: &str = \"{sig}\";\n",
        dt   = format_timestamp(a.ceremony_timestamp),
        op   = a.operator_name,
        year = a.production_data_year,
        mh   = a.master_hash,
        fr   = format_flakes_constant(a.base_reward_flakes),
        opl  = a.base_reward_opl,
        ts   = a.ceremony_timestamp,
        pk   = a.operator_public_key,
        sig  = a.operator_signature,
    );
    let path = dir.join("genesis_params.rs");
    std::fs::write(&path, content)?;
    println!("  Written: {}", path.display());
    Ok(())
}

fn write_verification_txt(dir: &Path, a: &GenesisAttestation) -> std::io::Result<()> {
    let mut lines: Vec<String> = Vec::new();
    lines.push("OPOLYS GENESIS CEREMONY — INDEPENDENT VERIFICATION GUIDE".into());
    lines.push("=".repeat(60));
    lines.push(format!("Ceremony time    : {}", format_timestamp(a.ceremony_timestamp)));
    lines.push(format!("Ceremony duration: {}s",
        (a.ceremony_end_ms.saturating_sub(a.ceremony_start_ms)) / 1000));
    lines.push(format!("Operator         : {}", a.operator_name));
    lines.push(format!("Operator pubkey  : {}", a.operator_public_key));
    lines.push(format!("Master hash      : {}", a.master_hash));
    lines.push(String::new());

    lines.push("DATA NOTE".into());
    lines.push("-".repeat(40));
    lines.push(format!("  Production year : {} (most recent USGS/WGC annual figure)", a.production_data_year));
    lines.push("  Gold mine production is measured and published annually.".into());
    lines.push("  Using the most recent published figure is correct and honest.".into());
    lines.push("  Gold spot price was fetched live at ceremony time.".into());
    if a.price_fetch_spread_ms > 0 {
        lines.push(format!("  Price source spread: {:.1}s between earliest and latest fetch.",
            a.price_fetch_spread_ms as f64 / 1_000.0));
    }
    lines.push(String::new());

    lines.push("STEP 1 — VERIFY PRODUCTION SOURCES".into());
    lines.push("-".repeat(40));
    for s in &a.production_sources {
        let v = s.extracted_value.map(|v| format!("{:.1} t", v)).unwrap_or_else(|| "—".into());
        let ts = if s.fetched_at_ms > 0 {
            format!("fetched_at={}", s.fetched_at_ms)
        } else { "manual".into() };
        lines.push(format!("  {:22} {:12}  [{}]  {}  hash:{}...",
            s.name, v, s.status, ts, &s.raw_response_hash[..12]));
    }
    lines.push(String::new());

    lines.push("STEP 2 — VERIFY PRICE SOURCES (LIVE AT CEREMONY)".into());
    lines.push("-".repeat(40));
    for s in &a.price_sources {
        let v = s.extracted_value.map(|v| format!("${:.2}/oz", v)).unwrap_or_else(|| "—".into());
        let ts = if s.fetched_at_ms > 0 {
            format!("fetched_at_ms={}", s.fetched_at_ms)
        } else { "manual".into() };
        lines.push(format!("  {:22} {:14}  [{}]  {}  hash:{}...",
            s.name, v, s.status, ts, &s.raw_response_hash[..12]));
    }
    lines.push(String::new());

    lines.push("STEP 3 — VERIFY DERIVATION".into());
    lines.push("-".repeat(40));
    for step in &a.derivation_steps { lines.push(format!("  {}", step)); }
    lines.push(String::new());

    lines.push("STEP 4 — VERIFY MASTER HASH".into());
    lines.push("-".repeat(40));
    lines.push("  1. Open genesis_attestation.json".into());
    lines.push("  2. Set 'master_hash' to \"\" and 'operator_signature' to \"\"".into());
    lines.push("  3. Serialize back to compact JSON preserving key order:".into());
    lines.push("     python3 -c \"import json; d=json.load(open('genesis_attestation.json')); \\ ".into());
    lines.push("               d['master_hash']=''; d['operator_signature']=''; \\ ".into());
    lines.push("               print(json.dumps(d, separators=(',',':')))\"".into());
    lines.push("  4. Compute Blake3-256 of that output".into());
    lines.push(format!("  5. Must equal: {}", a.master_hash));
    lines.push(String::new());

    lines.push("STEP 5 — VERIFY OPERATOR SIGNATURE".into());
    lines.push("-".repeat(40));
    lines.push("  The operator signed the master hash bytes with their ed25519 private key.".into());
    lines.push("  1. Hex-decode master_hash → 32 bytes (the signed message)".into());
    lines.push("  2. Hex-decode operator_public_key → 32 bytes (ed25519 verifying key)".into());
    lines.push("  3. Hex-decode operator_signature → 64 bytes".into());
    lines.push("  4. ed25519_verify(pubkey, message=master_hash_bytes, sig) → must succeed".into());
    lines.push("  This proves the ceremony was attested by the holder of the operator key.".into());
    lines.push("  The operator public key is stored in the genesis block.".into());
    lines.push(String::new());

    lines.push("RESULT".into());
    lines.push("-".repeat(40));
    lines.push(format!("  Median production : {:.1} t/yr  ({} data)", a.median_production_tonnes, a.production_data_year));
    lines.push(format!("  Median price      : ${:.2}/oz  ({} USD cents)",
        a.median_price_usd_cents as f64 / 100.0, a.median_price_usd_cents));
    lines.push(format!("  Blocks per year   : {}", a.blocks_per_year));
    lines.push(format!("  BASE_REWARD       : {} OPL ({} Flakes)",
        a.base_reward_opl, a.base_reward_flakes));

    let path = dir.join("genesis_verification.txt");
    std::fs::write(&path, lines.join("\n") + "\n")?;
    println!("  Written: {}", path.display());
    Ok(())
}

fn write_outputs(dir: &Path, a: &GenesisAttestation) {
    std::fs::create_dir_all(dir).ok();
    println!("\n── WRITING OUTPUT FILES ────────────────────────────");
    write_attestation_json(dir, a).expect("failed to write genesis_attestation.json");
    write_params_rs(dir, a).expect("failed to write genesis_params.rs");
    write_verification_txt(dir, a).expect("failed to write genesis_verification.txt");
}

// ─── Ceremony logic ───────────────────────────────────────────────────────────

async fn run_ceremony(cli: Cli) {
    let ceremony_start_ms = now_ms();
    let ceremony_timestamp = ceremony_start_ms / 1_000;

    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║       OPOLYS GENESIS CEREMONY                    ║");
    println!("║  Operator : {:37}║", cli.operator);
    println!("║  Window   : {} minutes max                       ║", CEREMONY_WINDOW_SECS / 60);
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    // ── Production year ──────────────────────────────────────────────────────
    let production_year = cli.production_year.unwrap_or_else(|| {
        // Default: previous calendar year (USGS/WGC data lags by ~1 year)
        let y = format_timestamp(ceremony_timestamp);
        y[..4].parse::<u32>().unwrap_or(2024).saturating_sub(1)
    });

    // ── Operator signing key ─────────────────────────────────────────────────
    let (signing_key, operator_public_key) = if cli.dry_run {
        let sk = SigningKey::from_bytes(&DRY_RUN_KEY_SEED);
        let pk = hex::encode(sk.verifying_key().as_bytes());
        println!("[DRY RUN] Using test-only signing key (seed=[42;32]).");
        (sk, pk)
    } else {
        println!("── OPERATOR KEY ────────────────────────────────────");
        load_or_generate_signing_key(cli.operator_key_file.as_deref(), &cli.output_dir)
    };

    // ── Dry-run shortcut ─────────────────────────────────────────────────────
    if cli.dry_run {
        println!("[DRY RUN] Using hard-coded {} USGS/WGC/LBMA actuals.", DRY_RUN_PROD_YEAR);
        println!("  Production: {:.1} t/yr", DRY_RUN_PROD_TONNES);
        println!("  Price     : ${:.2}/oz", DRY_RUN_PRICE_USD_OZ);

        let blocks_per_year = compute_blocks_per_year();
        let annual_oz = DRY_RUN_PROD_TONNES * TROY_OZ_PER_TONNE;
        let base_reward_opl = (annual_oz / blocks_per_year as f64).floor() as u64;
        let base_reward_flakes = base_reward_opl * FLAKES_PER_OPL;
        let price_cents = (DRY_RUN_PRICE_USD_OZ * 100.0).round() as u64;
        let dry_hash = hex::encode(blake3::hash(b"dry-run").as_bytes());

        let make = |def: &SourceDef, val: f64| SourceResult {
            name: def.name.to_string(),
            url: def.url.to_string(),
            fetched_at_ms: 0,
            raw_response_hash: dry_hash.clone(),
            extracted_value: Some(val),
            status: "dry-run".to_string(),
        };

        let mut attestation = GenesisAttestation {
            ceremony_start_ms,
            ceremony_end_ms: now_ms(),
            ceremony_timestamp,
            operator_name: cli.operator.clone(),
            operator_public_key: operator_public_key.clone(),
            operator_signature: String::new(),
            production_data_year: DRY_RUN_PROD_YEAR,
            production_sources: PROD_SOURCES.iter().map(|s| make(s, DRY_RUN_PROD_TONNES)).collect(),
            price_sources: PRICE_SOURCES.iter().map(|s| make(s, DRY_RUN_PRICE_USD_OZ)).collect(),
            price_fetch_spread_ms: 0,
            median_production_tonnes: DRY_RUN_PROD_TONNES,
            median_price_usd_cents: price_cents,
            blocks_per_year,
            base_reward_opl,
            base_reward_flakes,
            derivation_steps: build_derivation_steps(
                DRY_RUN_PROD_TONNES, blocks_per_year, annual_oz, base_reward_opl, base_reward_flakes,
            ),
            master_hash: String::new(),
        };
        attestation.master_hash = compute_master_hash(&attestation);

        if operator_confirm(&attestation, true) {
            attestation.operator_signature = sign_master_hash(&signing_key, &attestation.master_hash);
            attestation.ceremony_end_ms = now_ms();
            write_outputs(&cli.output_dir, &attestation);
            println!("\nCeremony complete.");
            println!("Verify with: genesis-ceremony --dry-run");
        } else {
            println!("Ceremony aborted by operator.");
        }
        return;
    }

    // ── Manual or auto mode ──────────────────────────────────────────────────
    let manual = cli.manual;
    if manual {
        println!("[MANUAL MODE] You will be prompted for every value.");
    } else {
        println!("[AUTO MODE] Fetching {} sources concurrently ({}s timeout each)...",
            PROD_SOURCES.len() + PRICE_SOURCES.len(), FETCH_TIMEOUT_SECS);
    }

    // ── Production data ──────────────────────────────────────────────────────
    println!("\n── PRODUCTION DATA ({} annual figure) ──────────────", production_year);
    println!("   Annual mine production is published once per year.");
    println!("   Using {} USGS/WGC data (most recent available).", production_year);

    let prod_fetch = if manual {
        PROD_SOURCES.iter().map(|s| (s.name.to_string(), empty_hash(), None, 0u64)).collect()
    } else {
        run_fetch_phase(PROD_SOURCES, parse_production_tonnes).await
    };

    let mut prod_results = collect_source_results(
        PROD_SOURCES, prod_fetch, manual,
        "annual mine production", "metric tonnes", "3630",
        PROD_MIN_TONNES, PROD_MAX_TONNES,
    );

    let prod_values: Vec<f64> = prod_results.iter().filter_map(|r| r.extracted_value).collect();
    if prod_values.len() < MIN_PROD_SOURCES {
        eprintln!("\nERROR: Only {}/{} production sources succeeded (need >= {}).",
            prod_values.len(), PROD_SOURCES.len(), MIN_PROD_SOURCES);
        eprintln!("Re-run with --manual to enter values by hand.");
        std::process::exit(1);
    }

    let median_prod = trimmed_median(prod_values);
    apply_outlier_flags(&mut prod_results, median_prod);
    println!("\n  Median production (trimmed): {:.1} t/yr", median_prod);

    // ── Price data ───────────────────────────────────────────────────────────
    println!("\n── PRICE DATA (live at ceremony time) ──────────────");
    println!("   Fetching live spot price — NOT daily average, NOT historical.");

    let price_fetch = if manual {
        PRICE_SOURCES.iter().map(|s| (s.name.to_string(), empty_hash(), None, 0u64)).collect()
    } else {
        run_fetch_phase(PRICE_SOURCES, parse_price_usd_oz).await
    };

    let mut price_results = collect_source_results(
        PRICE_SOURCES, price_fetch, manual,
        "live gold spot price", "USD per troy oz", "2386.00",
        PRICE_MIN_USD_OZ, PRICE_MAX_USD_OZ,
    );

    let price_values: Vec<f64> = price_results.iter().filter_map(|r| r.extracted_value).collect();
    let spread_ms = price_fetch_spread_ms(&price_results);

    let median_price_oz = if !price_values.is_empty() {
        let m = trimmed_median(price_values);
        apply_outlier_flags(&mut price_results, m);
        if spread_ms > PRICE_SPREAD_WARN_MS {
            println!("\n  !! WARNING: price sources fetched {:.0}s apart (>{:.0}s threshold)",
                spread_ms as f64 / 1_000.0, PRICE_SPREAD_WARN_MS as f64 / 1_000.0);
            println!("  !! Consider aborting and re-running if spread is unacceptable.");
        }
        m
    } else {
        println!("  Warning: no price sources succeeded. Recording 0.");
        0.0
    };
    let price_cents = (median_price_oz * 100.0).round() as u64;
    println!("\n  Median price (trimmed): ${:.2}/oz", median_price_oz);

    // ── Compute BASE_REWARD ──────────────────────────────────────────────────
    let blocks_per_year = compute_blocks_per_year();
    let annual_oz = median_prod * TROY_OZ_PER_TONNE;
    let base_reward_opl = (annual_oz / blocks_per_year as f64).floor() as u64;
    let base_reward_flakes = base_reward_opl * FLAKES_PER_OPL;

    // ── Assemble attestation ─────────────────────────────────────────────────
    let mut attestation = GenesisAttestation {
        ceremony_start_ms,
        ceremony_end_ms: now_ms(),
        ceremony_timestamp,
        operator_name: cli.operator.clone(),
        operator_public_key: operator_public_key.clone(),
        operator_signature: String::new(),
        production_data_year: production_year,
        production_sources: prod_results,
        price_sources: price_results,
        price_fetch_spread_ms: spread_ms,
        median_production_tonnes: median_prod,
        median_price_usd_cents: price_cents,
        blocks_per_year,
        base_reward_opl,
        base_reward_flakes,
        derivation_steps: build_derivation_steps(
            median_prod, blocks_per_year, annual_oz, base_reward_opl, base_reward_flakes,
        ),
        master_hash: String::new(),
    };
    attestation.master_hash = compute_master_hash(&attestation);

    // ── Operator confirmation ────────────────────────────────────────────────
    if !operator_confirm(&attestation, false) {
        println!("Ceremony aborted by operator. No files written.");
        std::process::exit(0);
    }

    // ── Sign and write ───────────────────────────────────────────────────────
    attestation.operator_signature = sign_master_hash(&signing_key, &attestation.master_hash);
    attestation.ceremony_end_ms = now_ms();

    write_outputs(&cli.output_dir, &attestation);
    println!("\nCeremony complete. Duration: {}s",
        (attestation.ceremony_end_ms - attestation.ceremony_start_ms) / 1_000);
    println!("Verify independently using genesis_verification.txt");
}

// ─── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = tokio::time::timeout(
        Duration::from_secs(CEREMONY_WINDOW_SECS),
        run_ceremony(cli),
    ).await;

    if result.is_err() {
        eprintln!();
        eprintln!("╔══════════════════════════════════════════════════╗");
        eprintln!("║  CEREMONY WINDOW EXCEEDED ({} minutes)           ║", CEREMONY_WINDOW_SECS / 60);
        eprintln!("║  All data must reflect the same market moment.   ║");
        eprintln!("║  Abort and restart the ceremony from the top.    ║");
        eprintln!("╚══════════════════════════════════════════════════╝");
        std::process::exit(2);
    }
}
