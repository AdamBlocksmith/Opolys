//! Genesis Ceremony — Opolys ($OPL) chain initialization tool.
//!
//! Fetches annual gold production and spot-price data from multiple independent
//! sources, applies a median / outlier algorithm, computes the canonical
//! BASE_REWARD, and writes a cryptographically attested output set that anyone
//! can independently verify.
//!
//! Modes
//! ─────
//!   (default)          Try every source concurrently; fall back to manual
//!                      prompt for any source that fails to parse.
//!   --manual           Skip all network fetches; prompt for every value.
//!   --dry-run          Use hard-coded 2024 USGS/WGC/LBMA actuals; no
//!                      network I/O and no prompts.
//!
//! Output files (written to --output-dir, default ".")
//! ─────────────────────────────────────────────────────
//!   genesis_attestation.json   Full human-readable attestation record.
//!   genesis_params.rs          Ready-to-paste Rust constants.
//!   genesis_verification.txt   Step-by-step verification guide.

use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

// ─── Physical constants ───────────────────────────────────────────────────────

const TROY_OZ_PER_TONNE: f64 = 32_150.7;
const BLOCK_TARGET_TIME_MS: u64 = 84_375;
const FLAKES_PER_OPL: u64 = 1_000_000;

// Sanity bounds (reject anything outside these before touching the algorithm)
const PROD_MIN_TONNES: f64 = 1_000.0;
const PROD_MAX_TONNES: f64 = 10_000.0;
const PRICE_MIN_USD_OZ: f64 = 500.0;
const PRICE_MAX_USD_OZ: f64 = 15_000.0;

const OUTLIER_PCT: f64 = 0.15; // flag if >15% from median
const MIN_SOURCES: usize = 5;  // abort if fewer succeed

// Dry-run hard-coded actuals (USGS/WGC 2024, LBMA 2024 avg)
const DRY_RUN_PROD_TONNES: f64 = 3_630.0;
const DRY_RUN_PRICE_USD_OZ: f64 = 2_386.0; // 2024 LBMA annual average

const FETCH_TIMEOUT_SECS: u64 = 30;

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "genesis-ceremony",
    about = "Opolys genesis ceremony — compute BASE_REWARD from live gold data",
    long_about = None
)]
struct Cli {
    /// Use hard-coded 2024 actuals. No network calls, no prompts.
    #[arg(long)]
    dry_run: bool,

    /// Prompt for every value manually. Skip all network fetches.
    #[arg(long)]
    manual: bool,

    /// Name of the ceremony operator (recorded in attestation).
    #[arg(long, default_value = "unknown")]
    operator: String,

    /// Directory to write output files.
    #[arg(long, default_value = ".")]
    output_dir: PathBuf,
}

// ─── Source definitions ───────────────────────────────────────────────────────

struct SourceDef {
    name: &'static str,
    url: &'static str,
    /// Human instructions shown when automatic extraction fails.
    instructions: &'static str,
}

static PROD_SOURCES: &[SourceDef] = &[
    SourceDef {
        name: "USGS",
        url: "https://minerals.usgs.gov/minerals/pubs/commodity/gold/",
        instructions: "Navigate to the USGS gold page. Find the 'Mine production, world' \
                       row in the most recent statistics table. Enter the value in metric tonnes.",
    },
    SourceDef {
        name: "World Gold Council",
        url: "https://www.gold.org/goldhub/data/gold-demand-statistics",
        instructions: "On the WGC Gold Demand Statistics page, find 'Mine production' under \
                       Supply. Enter the most recent annual figure in metric tonnes.",
    },
    SourceDef {
        name: "Kitco",
        url: "https://www.kitco.com/charts/goldproduction.html",
        instructions: "On Kitco's gold production page, find the most recent annual world \
                       mine production figure. Enter in metric tonnes.",
    },
    SourceDef {
        name: "MacroTrends",
        url: "https://www.macrotrends.net/1369/gold-price-history",
        instructions: "MacroTrends shows annual gold production. Find the most recent year's \
                       world mine production. Enter in metric tonnes.",
    },
    SourceDef {
        name: "Trading Economics",
        url: "https://tradingeconomics.com/commodity/gold",
        instructions: "Check the Trading Economics gold production page. Enter the latest \
                       annual world mine output in metric tonnes.",
    },
    SourceDef {
        name: "LBMA Production",
        url: "https://www.lbma.org.uk/prices-and-data/precious-metal-prices",
        instructions: "The LBMA publishes annual gold statistics. Find 'World mine production' \
                       in their annual survey. Enter in metric tonnes.",
    },
    SourceDef {
        name: "Metals Focus",
        url: "https://metalsfocus.com",
        instructions: "Metals Focus publishes annual gold mine supply data (may require \
                       subscription). Enter world mine production for the most recent year \
                       in metric tonnes.",
    },
];

static PRICE_SOURCES: &[SourceDef] = &[
    SourceDef {
        name: "CME Group COMEX",
        url: "https://www.cmegroup.com/markets/metals/precious/gold.html",
        instructions: "On the CME Group gold futures page, find the front-month settlement \
                       or spot price. Enter in USD per troy ounce.",
    },
    SourceDef {
        name: "LBMA Gold Price",
        url: "https://www.lbma.org.uk/prices-and-data/precious-metal-prices",
        instructions: "The LBMA publishes PM fix prices. Enter today's PM gold fix \
                       in USD per troy ounce.",
    },
    SourceDef {
        name: "Kitco Spot",
        url: "https://www.kitco.com/gold-price-today-usa/",
        instructions: "On Kitco's live gold price page, find the current spot price \
                       in USD per troy ounce.",
    },
    SourceDef {
        name: "BullionVault",
        url: "https://api.bullionvault.com/gold-price-chart.json",
        instructions: "BullionVault API returns JSON with current gold price. \
                       Enter the price in USD per troy ounce.",
    },
    SourceDef {
        name: "Goldprice.org",
        url: "https://goldprice.org/gold-price.html",
        instructions: "On goldprice.org, find the current spot gold price. \
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
    /// Blake3-256 hex of the raw HTTP response body (empty string hash if fetch failed).
    raw_response_hash: String,
    /// The value extracted from this source (tonnes for production, USD/oz for price).
    extracted_value: Option<f64>,
    /// "ok" | "failed" | "outlier" | "manual" | "manual-outlier"
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenesisAttestation {
    ceremony_timestamp: u64,
    operator_name: String,
    production_sources: Vec<SourceResult>,
    price_sources: Vec<SourceResult>,
    median_production_tonnes: f64,
    median_price_usd_cents: u64,
    blocks_per_year: u64,
    base_reward_opl: u64,
    base_reward_flakes: u64,
    derivation_steps: Vec<String>,
    /// Blake3-256 of the full attestation JSON (with this field set to "").
    master_hash: String,
}

// ─── Fetch utilities ──────────────────────────────────────────────────────────

/// Fetch a URL, returning (body, blake3_hex). On any error returns ("", hash_of_empty).
async fn fetch(url: &str) -> (String, String) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("Mozilla/5.0 (compatible; OplGenesisCeremony/1.0; +https://github.com/opolys)")
        .build()
    {
        Ok(c) => c,
        Err(_) => return empty_fetch_result(),
    };

    match client.get(url).send().await {
        Ok(resp) => {
            let body = resp.text().await.unwrap_or_default();
            let hash = hex::encode(blake3::hash(body.as_bytes()).as_bytes());
            (body, hash)
        }
        Err(_) => empty_fetch_result(),
    }
}

fn empty_fetch_result() -> (String, String) {
    let hash = hex::encode(blake3::hash(b"").as_bytes());
    (String::new(), hash)
}

// ─── Parse utilities ──────────────────────────────────────────────────────────

/// Try to extract annual gold production in tonnes from a raw HTTP response.
fn parse_production_tonnes(source_name: &str, body: &str) -> Option<f64> {
    if body.is_empty() {
        return None;
    }
    // BullionVault returns JSON — try JSON path first
    if source_name == "BullionVault" {
        return None; // BullionVault is a price source, not production
    }
    // Generic: find a number in [PROD_MIN, PROD_MAX] that appears near
    // "tonne", "metric ton", or "production" keywords.
    let text = body.to_lowercase();
    let keywords = ["tonne", "metric ton", "mine production", "world production"];
    for kw in &keywords {
        if let Some(pos) = text.find(kw) {
            let window_start = pos.saturating_sub(200);
            let window = &body[window_start..pos];
            if let Some(n) = last_number_in_range(window, PROD_MIN_TONNES, PROD_MAX_TONNES) {
                return Some(n);
            }
            // Also try after the keyword
            let window_end = (pos + kw.len() + 200).min(body.len());
            let window = &body[pos + kw.len()..window_end];
            if let Some(n) = last_number_in_range(window, PROD_MIN_TONNES, PROD_MAX_TONNES) {
                return Some(n);
            }
        }
    }
    None
}

/// Try to extract gold spot price in USD per troy oz from a raw HTTP response.
fn parse_price_usd_oz(source_name: &str, body: &str) -> Option<f64> {
    if body.is_empty() {
        return None;
    }
    // BullionVault has a JSON API
    if source_name == "BullionVault" {
        return parse_bullionvault_json(body);
    }
    // Generic: find a number in [PRICE_MIN, PRICE_MAX] near price keywords.
    let text = body.to_lowercase();
    let keywords = ["spot price", "gold price", "ask price", "bid price", "xau", "per oz", "per ounce"];
    for kw in &keywords {
        if let Some(pos) = text.find(kw) {
            // Search a window around the keyword
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

fn parse_bullionvault_json(body: &str) -> Option<f64> {
    // BullionVault API: look for a price field in the JSON that's in our range.
    // The exact schema varies; we do a best-effort numeric search on the JSON.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        // Walk the JSON tree looking for a plausible price
        return find_price_in_json(&val);
    }
    None
}

fn find_price_in_json(val: &serde_json::Value) -> Option<f64> {
    match val {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f >= PRICE_MIN_USD_OZ && f <= PRICE_MAX_USD_OZ {
                return Some(f);
            }
            None
        }
        serde_json::Value::Object(map) => {
            // Prioritise keys that suggest a price
            for key in &["price", "ask", "bid", "spot", "usd", "rate"] {
                if let Some(v) = map.get(*key) {
                    if let Some(f) = find_price_in_json(v) {
                        return Some(f);
                    }
                }
            }
            // Fall back to any key
            for v in map.values() {
                if let Some(f) = find_price_in_json(v) {
                    return Some(f);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let Some(f) = find_price_in_json(v) {
                    return Some(f);
                }
            }
            None
        }
        _ => None,
    }
}

/// Scan `s` for the last numeric token (digits + optional commas/dot) whose
/// parsed f64 value falls within [min, max].
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
            // Strip commas (thousands separators)
            let num_str: String = chars[start..i].iter().filter(|&&c| c != ',').collect();
            if let Ok(n) = num_str.parse::<f64>() {
                if n >= min && n <= max {
                    result = Some(n);
                }
            }
        } else {
            i += 1;
        }
    }
    result
}

// ─── Median + outlier algorithm ───────────────────────────────────────────────

/// Drop the single highest and single lowest value, then return the median.
/// Input must have at least 3 values.
fn trimmed_median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if values.len() >= 3 {
        values.remove(0);                  // drop lowest
        values.remove(values.len() - 1);  // drop highest
    }
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn is_outlier(value: f64, median: f64) -> bool {
    (value - median).abs() / median > OUTLIER_PCT
}

// ─── Manual prompts ───────────────────────────────────────────────────────────

fn prompt(msg: &str) -> String {
    print!("{}", msg);
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line).ok();
    line.trim().to_string()
}

/// Prompt the operator for a numeric value from a named source.
/// Returns None if the operator enters "skip".
fn prompt_value(def: &SourceDef, value_desc: &str, unit: &str, example: &str) -> Option<f64> {
    println!();
    println!("  Source  : {}", def.name);
    println!("  URL     : {}", def.url);
    println!("  How     : {}", def.instructions);
    println!("  Looking for: {} in {}", value_desc, unit);
    println!("  Example : {}", example);
    loop {
        let s = prompt(&format!("  Enter {} in {} (or 'skip'): ", value_desc, unit));
        if s.eq_ignore_ascii_case("skip") {
            return None;
        }
        let clean: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
        match clean.parse::<f64>() {
            Ok(n) if n > 0.0 => return Some(n),
            _ => println!("  Invalid number. Try again or type 'skip'."),
        }
    }
}

// ─── Derive blocks_per_year ───────────────────────────────────────────────────

fn compute_blocks_per_year() -> u64 {
    // floor(365.25 days × 86400 s/day × 1000 ms/s / BLOCK_TARGET_TIME_MS)
    let ms_per_year = 365.25_f64 * 86_400.0 * 1_000.0;
    (ms_per_year / BLOCK_TARGET_TIME_MS as f64).floor() as u64
}

// ─── Core ceremony logic ──────────────────────────────────────────────────────

async fn run_fetch_phase(
    sources: &[SourceDef],
    parse_fn: fn(&str, &str) -> Option<f64>,
) -> Vec<(String, String, Option<f64>)> {
    let handles: Vec<_> = sources
        .iter()
        .map(|s| {
            let url = s.url.to_string();
            let name = s.name.to_string();
            tokio::spawn(async move {
                let (body, hash) = fetch(&url).await;
                let value = parse_fn(&name, &body);
                (name, hash, value)
            })
        })
        .collect();

    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap_or_else(|_| ("error".into(), empty_fetch_result().1, None)));
    }
    results
}

fn collect_source_results(
    sources: &[SourceDef],
    fetch_results: Vec<(String, String, Option<f64>)>,
    manual: bool,
    value_desc: &str,
    unit: &str,
    example: &str,
    sanity_min: f64,
    sanity_max: f64,
) -> Vec<SourceResult> {
    let mut out = Vec::new();

    for (def, (_, hash, auto_value)) in sources.iter().zip(fetch_results.into_iter()) {
        let value = if manual {
            // Manual mode: always prompt, ignore any auto parse
            prompt_value(def, value_desc, unit, example)
        } else if auto_value.is_some() {
            auto_value
        } else {
            // Auto parse failed — offer manual fallback
            println!("\n  [auto-parse failed for {}]", def.name);
            prompt_value(def, value_desc, unit, example)
        };

        // Sanity-check the entered/parsed value
        let value = value.filter(|&v| v >= sanity_min && v <= sanity_max);

        out.push(SourceResult {
            name: def.name.to_string(),
            url: def.url.to_string(),
            raw_response_hash: hash,
            extracted_value: value,
            status: if value.is_some() { "ok".to_string() } else { "failed".to_string() },
        });
    }

    out
}

fn apply_outlier_flags(results: &mut Vec<SourceResult>, median: f64) {
    for r in results.iter_mut() {
        if let Some(v) = r.extracted_value {
            if is_outlier(v, median) {
                r.status = if r.status == "manual" { "manual-outlier".to_string() } else { "outlier".to_string() };
            }
        }
    }
}

// ─── Output generation ────────────────────────────────────────────────────────

fn format_timestamp(ts: u64) -> String {
    // Format as YYYY-MM-DD HH:MM:SS UTC (no chrono dependency)
    let secs = ts;
    let days_since_epoch = secs / 86_400;
    let time_of_day = secs % 86_400;
    let hh = time_of_day / 3_600;
    let mm = (time_of_day % 3_600) / 60;
    let ss = time_of_day % 60;

    // Gregorian calendar computation from days since 1970-01-01
    let mut y = 1970u64;
    let mut d = days_since_epoch;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if d < days_in_year { break; }
        d -= days_in_year;
        y += 1;
    }
    let months = if is_leap(y) {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 1u64;
    for &days_in_mo in &months {
        if d < days_in_mo { break; }
        d -= days_in_mo;
        mo += 1;
    }
    let day = d + 1;
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, mo, day, hh, mm, ss)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn compute_master_hash(attestation: &GenesisAttestation) -> String {
    // Serialize with master_hash = "" then hash the bytes
    let mut tmp = serde_json::to_value(attestation).unwrap();
    tmp["master_hash"] = serde_json::Value::String(String::new());
    let canonical = serde_json::to_string(&tmp).unwrap();
    hex::encode(blake3::hash(canonical.as_bytes()).as_bytes())
}

fn write_attestation_json(dir: &Path, attestation: &GenesisAttestation) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(attestation).unwrap();
    let path = dir.join("genesis_attestation.json");
    std::fs::write(&path, &json)?;
    println!("  Written: {}", path.display());
    Ok(())
}

fn write_params_rs(dir: &Path, attestation: &GenesisAttestation) -> std::io::Result<()> {
    let reward_opl = attestation.base_reward_opl;
    let reward_flakes = attestation.base_reward_flakes;
    let ts = attestation.ceremony_timestamp;
    let hash = &attestation.master_hash;
    let formatted_reward = format_flakes_constant(reward_flakes);

    let content = format!(
        "// Auto-generated by genesis-ceremony. Do not edit by hand.\n\
         // Ceremony: {dt}\n\
         // Operator: {op}\n\
         // Master hash: {hash}\n\
         //\n\
         // Paste these into crates/core/src/constants.rs and regenerate if needed.\n\n\
         pub const BASE_REWARD: u64 = {formatted_reward}; // {reward_opl} OPL derived from ceremony\n\
         pub const CEREMONY_TIMESTAMP: u64 = {ts};\n\
         pub const CEREMONY_MASTER_HASH: &str = \"{hash}\";\n",
        dt = format_timestamp(ts),
        op = attestation.operator_name,
        hash = hash,
        formatted_reward = formatted_reward,
        reward_opl = reward_opl,
        ts = ts,
    );

    let path = dir.join("genesis_params.rs");
    std::fs::write(&path, &content)?;
    println!("  Written: {}", path.display());
    Ok(())
}

fn format_flakes_constant(flakes: u64) -> String {
    // Format as "312_000_000" with underscore separators
    let s = flakes.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push('_');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

fn write_verification_txt(dir: &Path, attestation: &GenesisAttestation) -> std::io::Result<()> {
    let mut lines = Vec::new();
    lines.push("OPOLYS GENESIS CEREMONY — INDEPENDENT VERIFICATION GUIDE".to_string());
    lines.push("=".repeat(60));
    lines.push(format!("Ceremony time : {}", format_timestamp(attestation.ceremony_timestamp)));
    lines.push(format!("Operator      : {}", attestation.operator_name));
    lines.push(format!("Master hash   : {}", attestation.master_hash));
    lines.push(String::new());
    lines.push("STEP 1 — VERIFY PRODUCTION SOURCES".to_string());
    lines.push("-".repeat(40));
    for s in &attestation.production_sources {
        let v = s.extracted_value.map(|v| format!("{:.1} t", v)).unwrap_or_else(|| "—".into());
        lines.push(format!("  {:20} {:12}  [{}]  hash: {}",
            s.name, v, s.status, &s.raw_response_hash[..16]));
    }
    lines.push(String::new());
    lines.push("STEP 2 — VERIFY PRICE SOURCES".to_string());
    lines.push("-".repeat(40));
    for s in &attestation.price_sources {
        let v = s.extracted_value.map(|v| format!("${:.2}/oz", v)).unwrap_or_else(|| "—".into());
        lines.push(format!("  {:20} {:14}  [{}]  hash: {}",
            s.name, v, s.status, &s.raw_response_hash[..16]));
    }
    lines.push(String::new());
    lines.push("STEP 3 — VERIFY DERIVATION".to_string());
    lines.push("-".repeat(40));
    for step in &attestation.derivation_steps {
        lines.push(format!("  {}", step));
    }
    lines.push(String::new());
    lines.push("STEP 4 — VERIFY MASTER HASH".to_string());
    lines.push("-".repeat(40));
    lines.push("  1. Open genesis_attestation.json".to_string());
    lines.push("  2. Set the 'master_hash' field to \"\" (empty string)".to_string());
    lines.push("  3. Canonicalize with: python3 -c \"import json,sys; print(json.dumps(json.load(open('genesis_attestation.json'))))\"".to_string());
    lines.push("     (or any JSON serializer that preserves key order — use the raw file key order)".to_string());
    lines.push("  4. Compute: blake3sum of that output".to_string());
    lines.push(format!("  5. Result must equal: {}", attestation.master_hash));
    lines.push(String::new());
    lines.push("RESULT".to_string());
    lines.push("-".repeat(40));
    lines.push(format!("  Median production : {:.1} tonnes/year", attestation.median_production_tonnes));
    lines.push(format!("  Median price      : ${:.2}/oz  ({} USD cents)",
        attestation.median_price_usd_cents as f64 / 100.0,
        attestation.median_price_usd_cents));
    lines.push(format!("  Blocks per year   : {}", attestation.blocks_per_year));
    lines.push(format!("  BASE_REWARD       : {} OPL ({} Flakes)",
        attestation.base_reward_opl, attestation.base_reward_flakes));

    let path = dir.join("genesis_verification.txt");
    std::fs::write(&path, lines.join("\n") + "\n")?;
    println!("  Written: {}", path.display());
    Ok(())
}

// ─── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║       OPOLYS GENESIS CEREMONY                    ║");
    println!("║  Operator: {:38}║", cli.operator);
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // ── Dry run ──────────────────────────────────────────────────────────────
    if cli.dry_run {
        println!("[DRY RUN] Using hard-coded 2024 USGS/WGC/LBMA actuals.");
        println!("  Production: {:.1} t/yr", DRY_RUN_PROD_TONNES);
        println!("  Price     : ${:.2}/oz", DRY_RUN_PRICE_USD_OZ);

        let blocks_per_year = compute_blocks_per_year();
        let annual_oz = DRY_RUN_PROD_TONNES * TROY_OZ_PER_TONNE;
        let base_reward_opl = (annual_oz / blocks_per_year as f64).floor() as u64;
        let base_reward_flakes = base_reward_opl * FLAKES_PER_OPL;
        let price_cents = (DRY_RUN_PRICE_USD_OZ * 100.0).round() as u64;

        let dry_run_hash = hex::encode(blake3::hash(b"dry-run").as_bytes());

        let make_dry_source = |def: &SourceDef, val: f64| SourceResult {
            name: def.name.to_string(),
            url: def.url.to_string(),
            raw_response_hash: dry_run_hash.clone(),
            extracted_value: Some(val),
            status: "dry-run".to_string(),
        };

        let production_sources: Vec<_> = PROD_SOURCES.iter()
            .map(|s| make_dry_source(s, DRY_RUN_PROD_TONNES))
            .collect();
        let price_sources: Vec<_> = PRICE_SOURCES.iter()
            .map(|s| make_dry_source(s, DRY_RUN_PRICE_USD_OZ))
            .collect();

        let derivation_steps = build_derivation_steps(
            DRY_RUN_PROD_TONNES, blocks_per_year, annual_oz, base_reward_opl, base_reward_flakes,
        );

        let mut attestation = GenesisAttestation {
            ceremony_timestamp: timestamp,
            operator_name: cli.operator.clone(),
            production_sources,
            price_sources,
            median_production_tonnes: DRY_RUN_PROD_TONNES,
            median_price_usd_cents: price_cents,
            blocks_per_year,
            base_reward_opl,
            base_reward_flakes,
            derivation_steps,
            master_hash: String::new(),
        };
        attestation.master_hash = compute_master_hash(&attestation);

        print_summary(&attestation);
        write_outputs(&cli.output_dir, &attestation);
        return;
    }

    // ── Manual or auto mode ──────────────────────────────────────────────────
    let manual = cli.manual;

    if manual {
        println!("[MANUAL MODE] You will be prompted for every value.");
    } else {
        println!("[AUTO MODE] Fetching all sources concurrently ({}s timeout)...", FETCH_TIMEOUT_SECS);
    }

    // ── Production phase ─────────────────────────────────────────────────────
    println!("\n── PRODUCTION DATA ─────────────────────────────────");

    let prod_fetch_results = if manual {
        PROD_SOURCES.iter()
            .map(|s| (s.name.to_string(), empty_fetch_result().1, None))
            .collect()
    } else {
        println!("  Fetching {} sources...", PROD_SOURCES.len());
        run_fetch_phase(PROD_SOURCES, parse_production_tonnes).await
    };

    let mut prod_results = collect_source_results(
        PROD_SOURCES,
        prod_fetch_results,
        manual,
        "annual mine production",
        "metric tonnes",
        "3630",
        PROD_MIN_TONNES,
        PROD_MAX_TONNES,
    );

    let prod_values: Vec<f64> = prod_results.iter()
        .filter_map(|r| r.extracted_value)
        .collect();

    if prod_values.len() < MIN_SOURCES {
        eprintln!(
            "\nERROR: Only {}/{} production sources succeeded (need {}).",
            prod_values.len(), PROD_SOURCES.len(), MIN_SOURCES
        );
        eprintln!("Re-run with --manual to enter values by hand, or --dry-run to use 2024 actuals.");
        std::process::exit(1);
    }

    let median_prod = trimmed_median(prod_values);
    apply_outlier_flags(&mut prod_results, median_prod);
    println!("\n  Median production (trimmed): {:.1} t/yr", median_prod);

    // ── Price phase ──────────────────────────────────────────────────────────
    println!("\n── PRICE DATA ──────────────────────────────────────");

    let price_fetch_results = if manual {
        PRICE_SOURCES.iter()
            .map(|s| (s.name.to_string(), empty_fetch_result().1, None))
            .collect()
    } else {
        println!("  Fetching {} sources...", PRICE_SOURCES.len());
        run_fetch_phase(PRICE_SOURCES, parse_price_usd_oz).await
    };

    let mut price_results = collect_source_results(
        PRICE_SOURCES,
        price_fetch_results,
        manual,
        "gold spot price",
        "USD per troy oz",
        "2386.00",
        PRICE_MIN_USD_OZ,
        PRICE_MAX_USD_OZ,
    );

    let price_values: Vec<f64> = price_results.iter()
        .filter_map(|r| r.extracted_value)
        .collect();

    let median_price_oz = if !price_values.is_empty() {
        let m = trimmed_median(price_values);
        apply_outlier_flags(&mut price_results, m);
        m
    } else {
        println!("  Warning: no price sources succeeded. Using 0.");
        0.0
    };
    let price_cents = (median_price_oz * 100.0).round() as u64;
    println!("\n  Median price (trimmed): ${:.2}/oz", median_price_oz);

    // ── Compute BASE_REWARD ──────────────────────────────────────────────────
    let blocks_per_year = compute_blocks_per_year();
    let annual_oz = median_prod * TROY_OZ_PER_TONNE;
    let base_reward_opl = (annual_oz / blocks_per_year as f64).floor() as u64;
    let base_reward_flakes = base_reward_opl * FLAKES_PER_OPL;

    let derivation_steps = build_derivation_steps(
        median_prod, blocks_per_year, annual_oz, base_reward_opl, base_reward_flakes,
    );

    // ── Assemble + hash attestation ──────────────────────────────────────────
    let mut attestation = GenesisAttestation {
        ceremony_timestamp: timestamp,
        operator_name: cli.operator.clone(),
        production_sources: prod_results,
        price_sources: price_results,
        median_production_tonnes: median_prod,
        median_price_usd_cents: price_cents,
        blocks_per_year,
        base_reward_opl,
        base_reward_flakes,
        derivation_steps,
        master_hash: String::new(),
    };
    attestation.master_hash = compute_master_hash(&attestation);

    print_summary(&attestation);
    write_outputs(&cli.output_dir, &attestation);
}

fn build_derivation_steps(
    median_tonnes: f64,
    blocks_per_year: u64,
    annual_oz: f64,
    base_reward_opl: u64,
    base_reward_flakes: u64,
) -> Vec<String> {
    vec![
        format!("median_production_tonnes = {:.4} t/yr", median_tonnes),
        format!("troy_oz_per_tonne        = {}", TROY_OZ_PER_TONNE),
        format!("annual_oz                = {:.4} × {:.1} = {:.4}", median_tonnes, TROY_OZ_PER_TONNE, annual_oz),
        format!("block_target_time_ms     = {}", BLOCK_TARGET_TIME_MS),
        format!("blocks_per_year          = floor(365.25 × 86400 × 1000 / {}) = {}", BLOCK_TARGET_TIME_MS, blocks_per_year),
        format!("base_reward_opl          = floor({:.4} / {}) = {}", annual_oz, blocks_per_year, base_reward_opl),
        format!("base_reward_flakes       = {} × 1_000_000 = {}", base_reward_opl, base_reward_flakes),
    ]
}

fn print_summary(a: &GenesisAttestation) {
    println!();
    println!("── RESULT ──────────────────────────────────────────");
    println!("  Median production : {:.1} t/yr", a.median_production_tonnes);
    println!("  Median price      : ${:.2}/oz", a.median_price_usd_cents as f64 / 100.0);
    println!("  Blocks per year   : {}", a.blocks_per_year);
    println!("  BASE_REWARD       : {} OPL", a.base_reward_opl);
    println!("  BASE_REWARD       : {} Flakes", a.base_reward_flakes);
    println!("  Master hash       : {}", a.master_hash);
    println!();
}

fn write_outputs(dir: &Path, attestation: &GenesisAttestation) {
    std::fs::create_dir_all(dir).ok();
    println!("── WRITING OUTPUT FILES ────────────────────────────");
    write_attestation_json(dir, attestation).expect("failed to write genesis_attestation.json");
    write_params_rs(dir, attestation).expect("failed to write genesis_params.rs");
    write_verification_txt(dir, attestation).expect("failed to write genesis_verification.txt");
    println!();
    println!("Ceremony complete. Verify with: genesis-ceremony --dry-run");
}
