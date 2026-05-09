use std::env;
use std::process;
use std::time::Instant;

use evo_omap::{
    CowDataset, HashBuffers, compute_epoch_seed,
    compute_epoch_seed_with_epoch_length_and_seed_material, current_num_threads, evo_omap_hash,
    evo_omap_hash_with_buffers, generate_dataset, mine, mine_parallel, verify, verify_light,
};

fn leading_zero_bits(hash: &[u8; 32]) -> u64 {
    hash.iter()
        .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
        .take_while(|&b| b == 0)
        .count() as u64
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("EVO-OMAP Proof-of-Work");
        eprintln!("Usage:");
        eprintln!(
            "  {} mine <header_hex> <height> <difficulty> [max_nonce] [--parallel [threads]]",
            args[0]
        );
        eprintln!(
            "  {} verify <header_hex> <height> <nonce> <difficulty> [light]",
            args[0]
        );
        eprintln!("  {} hash <header_hex> <height> <nonce>", args[0]);
        eprintln!("  {} seed <height> [seed_material_hex]", args[0]);
        eprintln!(
            "  {} bench <header_hex> <height> <difficulty> [iterations]",
            args[0]
        );
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  {} mine 00 0 1 1000000", args[0]);
        eprintln!("  {} mine 00 0 1 1000000 --parallel", args[0]);
        eprintln!("  {} verify 00 0 12345 1", args[0]);
        eprintln!("  {} bench 00 0 1 10", args[0]);
        process::exit(1);
    }

    match args[1].as_str() {
        "mine" => {
            let mut pos = 2;
            if args.len() < 5 {
                eprintln!(
                    "Usage: {} mine <header_hex> <height> <difficulty> [max_nonce] [--parallel [threads]]",
                    args[0]
                );
                process::exit(1);
            }
            let header_hex = &args[pos];
            pos += 1;
            let height: u64 = args[pos].parse().expect("Invalid height");
            pos += 1;
            let difficulty: u64 = args[pos].parse().expect("Invalid difficulty");
            pos += 1;
            let max_nonce: u64 = if pos < args.len() && !args[pos].starts_with("--") {
                let n = args[pos].parse().unwrap_or(10_000_000);
                pos += 1;
                n
            } else {
                10_000_000
            };

            let parallel = pos < args.len() && args[pos] == "--parallel";
            let num_threads = if pos + 1 < args.len() && args[pos] == "--parallel" {
                if args[pos + 1].parse::<usize>().is_ok() {
                    args[pos + 1].parse().unwrap()
                } else {
                    current_num_threads()
                }
            } else {
                current_num_threads()
            };

            let header = match hex::decode(header_hex) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Invalid hex: {}", e);
                    process::exit(1);
                }
            };

            println!("Mining...");
            println!("Header: {}", header_hex);
            println!("Height: {}", height);
            println!("Difficulty: {}", difficulty);
            println!("Max nonce attempts: {}", max_nonce);
            let expected_attempts = 1u64.checked_shl(difficulty as u32).unwrap_or(u64::MAX);
            if max_nonce < expected_attempts {
                println!(
                    "Warning: max_nonce ({}) may be too low for difficulty {}.",
                    max_nonce, difficulty
                );
                println!(
                    "Expected attempts needed: ~{}. Consider increasing max_nonce.",
                    expected_attempts
                );
                println!();
            }
            if parallel {
                println!("Parallel mining with ~{} threads", num_threads);
            }
            println!();

            let mine_start = Instant::now();
            let (result, attempts) = if parallel {
                mine_parallel(&header, height, difficulty, max_nonce, num_threads)
            } else {
                mine(&header, height, difficulty, max_nonce)
            };
            let mine_time = mine_start.elapsed();

            println!("Mining time: {:.3}s", mine_time.as_secs_f64());

            match result {
                Some(nonce) => {
                    println!("Found valid nonce: {}", nonce);
                    println!("Nonce (hex): 0x{:x}", nonce);
                    println!("Hash attempts: {}", attempts);
                    println!(
                        "Hashrate: {:.2} H/s",
                        attempts as f64 / mine_time.as_secs_f64()
                    );
                }
                None => {
                    println!("No valid nonce found in {} attempts.", attempts);
                    process::exit(1);
                }
            }
        }

        "verify" => {
            if args.len() < 6 {
                eprintln!(
                    "Usage: {} verify <header_hex> <height> <nonce> <difficulty> [light]",
                    args[0]
                );
                process::exit(1);
            }
            let header_hex = &args[2];
            let height: u64 = args[3].parse().expect("Invalid height");
            let nonce: u64 = args[4].parse().expect("Invalid nonce");
            let difficulty: u64 = args[5].parse().expect("Invalid difficulty");
            let light = args.len() > 6 && args[6] == "light";

            let header = match hex::decode(header_hex) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Invalid hex: {}", e);
                    process::exit(1);
                }
            };

            let valid = if light {
                println!("Verifying with light mode (cache-based)...");
                verify_light(&header, height, nonce, difficulty)
            } else {
                println!("Verifying with full mode (dataset-based)...");
                verify(&header, height, nonce, difficulty)
            };

            if valid {
                println!("Proof-of-work is VALID");
                println!("Epoch seed: {}", hex::encode(compute_epoch_seed(height).0));
            } else {
                println!("Proof-of-work is INVALID");
                process::exit(1);
            }
        }

        "hash" => {
            if args.len() < 5 {
                eprintln!("Usage: {} hash <header_hex> <height> <nonce>", args[0]);
                process::exit(1);
            }
            let header_hex = &args[2];
            let height: u64 = args[3].parse().expect("Invalid height");
            let nonce: u64 = args[4].parse().expect("Invalid nonce");

            let header = match hex::decode(header_hex) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Invalid hex: {}", e);
                    process::exit(1);
                }
            };

            let epoch_seed = compute_epoch_seed(height);
            let mut dataset = generate_dataset(&epoch_seed);
            let hash = evo_omap_hash(&mut dataset, &header, height, nonce);

            println!("Hash: {}", hex::encode(hash.0));
        }

        "seed" => {
            if args.len() < 3 {
                eprintln!("Usage: {} seed <height> [seed_material_hex]", args[0]);
                process::exit(1);
            }
            let height: u64 = args[2].parse().expect("Invalid height");
            let seed_material = if args.len() > 3 {
                match hex::decode(&args[3]) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("Invalid seed material hex: {}", e);
                        process::exit(1);
                    }
                }
            } else {
                Vec::new()
            };
            let seed = if seed_material.is_empty() {
                compute_epoch_seed(height)
            } else {
                compute_epoch_seed_with_epoch_length_and_seed_material(height, 1024, &seed_material)
            };
            println!("Epoch seed: {}", hex::encode(seed.0));
        }

        "bench" => {
            if args.len() < 5 {
                eprintln!(
                    "Usage: {} bench <header_hex> <height> <difficulty> [iterations]",
                    args[0]
                );
                process::exit(1);
            }
            let header_hex = &args[2];
            let height: u64 = args[3].parse().expect("Invalid height");
            let difficulty: u64 = args[4].parse().expect("Invalid difficulty");
            let iterations: u64 = if args.len() > 5 {
                args[5].parse().unwrap_or(10)
            } else {
                10
            };

            let header = match hex::decode(header_hex) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Invalid hex: {}", e);
                    process::exit(1);
                }
            };

            println!("EVO-OMAP Benchmark");
            println!("====================");
            println!("Header: {}", header_hex);
            println!("Height: {}", height);
            println!("Difficulty: {}", difficulty);
            println!();

            // Dataset generation benchmark
            let epoch_seed = compute_epoch_seed(height);
            let start = Instant::now();
            let dataset = generate_dataset(&epoch_seed);
            let ds_time = start.elapsed();
            println!("Dataset generation:");
            println!("  Time: {:.3} ms", ds_time.as_secs_f64() * 1000.0);
            println!();

            // Single hash benchmark
            let mut cow_dataset = CowDataset::new(&dataset);
            let mut buffers = HashBuffers::new();

            let hash_start = Instant::now();
            for nonce in 0..iterations {
                cow_dataset.reset();
                let pow_hash = evo_omap_hash_with_buffers(
                    &mut cow_dataset,
                    &header,
                    height,
                    nonce,
                    &mut buffers,
                );
                let _ = u64::from_be_bytes(pow_hash.0[..8].try_into().unwrap());
            }
            let hash_time = hash_start.elapsed();
            println!("Per-hash benchmark ({} iterations):", iterations);
            println!("  Total time: {:.3} ms", hash_time.as_secs_f64() * 1000.0);
            println!(
                "  Per hash: {:.3} ms",
                hash_time.as_secs_f64() * 1000.0 / iterations as f64
            );
            println!(
                "  Hashrate: {:.2} H/s",
                iterations as f64 / hash_time.as_secs_f64()
            );
            println!();

            // Light verification benchmark
            println!("Light verification ({} iterations):", iterations);
            let light_start = Instant::now();
            for nonce in 0..iterations {
                verify_light(&header, height, nonce, difficulty);
            }
            let light_time = light_start.elapsed();
            println!("  Total time: {:.3} ms", light_time.as_secs_f64() * 1000.0);
            println!(
                "  Per verification: {:.3} ms",
                light_time.as_secs_f64() * 1000.0 / iterations as f64
            );
            println!(
                "  Verification/s: {:.2}",
                iterations as f64 / light_time.as_secs_f64()
            );
            println!();

            // Full verification benchmark
            println!("Full verification ({} iterations):", iterations.min(5));
            let mut cow_ds_full = CowDataset::new(&dataset);
            let mut buffers_full = HashBuffers::new();
            let full_iterations = iterations.min(5);
            let full_start = Instant::now();
            for nonce in 0..full_iterations {
                cow_ds_full.reset();
                let pow_hash = evo_omap_hash_with_buffers(
                    &mut cow_ds_full,
                    &header,
                    height,
                    nonce,
                    &mut buffers_full,
                );
                let _valid = leading_zero_bits(&pow_hash.0) >= difficulty;
            }
            let full_time = full_start.elapsed();
            println!("  Total time: {:.3} ms", full_time.as_secs_f64() * 1000.0);
            println!(
                "  Per verification: {:.3} ms",
                full_time.as_secs_f64() * 1000.0 / full_iterations as f64
            );
            println!(
                "  Verification/s: {:.2}",
                full_iterations as f64 / full_time.as_secs_f64()
            );
        }

        _ => {
            eprintln!("Unknown command: {}", args[1]);
            eprintln!("Valid commands: mine, verify, hash, seed, bench");
            process::exit(1);
        }
    }
}
