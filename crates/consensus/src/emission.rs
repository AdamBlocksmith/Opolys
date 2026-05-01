//! # Block reward and emission schedule for Opolys.
//!
//! Opolys has **no fixed emission schedule** and no halvings. Instead, block
//! rewards emerge from chain state:
//!
//! - **Vein yield** = `1 + ln(target / hash_int)`, where `target = 2^(64-D) - 1`
//!   and D is the EVO-OMAP difficulty (leading zero bits). This uses f64::ln()
//!   with deterministic IEEE 754 rounding. Most blocks earn ~1-2x BASE_REWARD,
//!   with diminishing returns for higher yields.
//! - **Base reward** = `BASE_REWARD / effective_difficulty`. As difficulty
//!   rises, the per-block reward naturally declines — mimicking the
//!   diminishing returns of real-world gold extraction.
//! - **Refiner weight** = `Σ entry.stake × (1 + ln(1 + entry.age_years))`,
//!   computed per bond entry. Each entry has its own seniority clock, so
//!   older entries earn proportionally more, but the marginal gain diminishes
//!   logarithmically over time, preventing permanent dominance by early stakers.
//! - **Stake coverage** — the ratio of bonded stake to total issued supply —
//!   determines how much of each block reward flows to miners vs. refiners.
//!   At 0% coverage, all rewards go to miners; at 100%, all go to refiners.
//!
//! There is no governance body and no parameter votes. Fee markets and chain
//! state drive everything.

use opolys_core::{FlakeAmount, MIN_DIFFICULTY};


/// Compute the target value for EVO-OMAP difficulty in u64 space.
///
/// EVO-OMAP difficulty D means the 256-bit SHA3-256 hash must have at least
/// D leading zero bits. For vein yield, we compare the first 8 bytes of the
/// hash (as a u64) against a target value. A valid hash (with D leading
/// zero bits) will have its u64 portion in the range [0, 2^(64-D)-1].
///
/// Therefore: `target = 2^(64-D) - 1` for D ≤ 64.
/// For D > 64, the u64 is necessarily 0 (impossibly difficult), so we
/// return 0 to indicate no valid hash is possible.
pub fn difficulty_to_target(difficulty: u64) -> u64 {
    if difficulty == 0 || difficulty > 64 {
        return 0;
    }
    // 2^(64-D) - 1, computed with u128 to avoid overflow
    ((1u128 << (64 - difficulty as usize)) - 1) as u64
}

/// Compute the total block reward using vein yield.
///
/// Vein yield uses the sqrt-compressed natural logarithm: `vein_yield = 1 + sqrt(ln(target / hash_int))`.
/// The sqrt compression makes rich veins rare — most blocks earn ~1.5-2× the base,
/// with a 3× block happening every ~50 blocks and a 5× block essentially never.
/// This mirrors real gold: most ore is low-grade, good veins are weekly, bonanzas are once-in-a-career.
///
/// The target is derived from EVO-OMAP difficulty (leading zero bits):
/// `target = 2^(64-D) - 1`, so at difficulty 1 the target is u64::MAX
/// and at difficulty 20 the target is ~2^44.
pub fn compute_block_reward(base_reward: FlakeAmount, difficulty: u64, pow_hash_value: u64) -> FlakeAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    let base = base_reward / effective_difficulty;
    let yield_milli = compute_vein_yield(difficulty, pow_hash_value);
    // yield_milli is in thousandths (milli), so divide by 1000
    // Use u128 intermediate to avoid overflow
    ((base as u128 * yield_milli as u128) / 1000) as FlakeAmount
}

/// Compute the base block reward without vein yield.
///
/// This is the baseline reward every block earns, divided by difficulty.
/// It shrinks as the network's effective difficulty grows, following the
/// same economic logic as real gold: harder extraction → smaller yield.
pub fn compute_base_reward(base_reward: FlakeAmount, difficulty: u64) -> FlakeAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    base_reward / effective_difficulty
}

/// Compute vein yield in milli (× 1000). Returns an integer where
/// 1000 = 1.0x yield, 2000 = 2.0x yield, etc.
///
/// vein_yield = 1 + ln(target / hash_int),           in milli
/// where `target = 2^(64-D) - 1` and D is the EVO-OMAP difficulty
/// (number of leading zero bits required in the SHA3-256 hash).
///
/// Compressed via sqrt: `vein_yield = 1 + sqrt(ln(target / hash_int))`.
/// The sqrt compression makes rich veins rare — a 3× block happens roughly
/// once every 50 blocks (~75 minutes), and a 5× block is essentially never
/// seen in practice. This mirrors real gold: most ore is low-grade, good veins
/// are weekly, bonanzas are once-in-a-career.
///
/// Uses f64 ratio computation for overflow safety and precision.
/// The result is clamped to a minimum of 1000 (1.0x) — every valid block
/// earns at least the base reward.
pub fn compute_vein_yield(difficulty: u64, hash_int: u64) -> u64 {
    // hash_int == 0 means Refiner block — returns flat 1.0x yield by design
    // Refiners earn predictable steady income (BASE_REWARD / difficulty)
    // Miners earn variable income based on hash luck (1x to ~4x)
    // This distinction is intentional: vaults earn fees, miners earn ore
    if difficulty == 0 || hash_int == 0 {
        return 1000;
    }
    let target = difficulty_to_target(difficulty);
    if target == 0 {
        // Difficulty > 64: impossible for u64, return minimum
        return 1000;
    }
    if target <= hash_int {
        // Hash was at or above target — no bonus beyond minimum
        // This shouldn't happen for a valid PoW, but handle it safely
        return 1000;
    }
    // Compute sqrt(ln(target / hash_int)) using f64 for overflow safety.
    // The sqrt compression makes the yield distribution half-normal:
    // rare bonanzas, most blocks near the median.
    let ratio = target as f64 / hash_int as f64;
    let ln_val = ratio.ln().sqrt() * 1000.0;
    // vein_yield = 1 + sqrt(ln(ratio)), in milli
    1000u64.saturating_add(ln_val.round().max(0.0) as u64)
}

/// Natural log computation returning ln(x / 1000) × 1000, where x is in
/// milli units (1000 = 1.0, 2000 = 2.0, etc.).
///
/// Uses IEEE 754 double-precision internally for the log computation, then
/// rounds to the nearest integer. IEEE 754 guarantees deterministic results
/// across all platforms, making this safe for consensus.
///
/// For x < 1000 (sub-unity values), returns 0.
/// For x ≥ 1000, the result is accurate to within ±1 milli.
pub fn ln_milli(x: u64) -> u64 {
    if x < 1000 {
        return 0;
    }
    // Compute ln(x / 1000) × 1000 using f64::ln()
    // IEEE 754 guarantees deterministic results on all platforms.
    // Rounding to nearest integer ensures consensus agreement.
    let value = x as f64 / 1000.0;
    let result = value.ln() * 1000.0;
    result.round().max(0.0) as u64
}

/// Compute a refiner's share of the block reward based on their total weight
/// relative to all active refiners. Weight is the sum of per-entry weights.
///
/// Each entry's weight = `stake × (1 + ln(1 + age_years))`, giving a logarithmic
/// seniority bonus. Rewards are distributed proportionally to total weight.
pub fn compute_refiner_reward(
    block_reward: FlakeAmount,
    refiner_stake: FlakeAmount,
    refiner_age_years: u64,
    total_weight: FlakeAmount,
) -> FlakeAmount {
    if total_weight == 0 {
        return 0;
    }
    // Compute refiner weight using integer-only seniority
    let weight = compute_refiner_weight(refiner_stake, refiner_age_years);
    // u128 intermediate prevents overflow on large reward × weight products
    ((block_reward as u128 * weight as u128) / total_weight as u128) as FlakeAmount
}

/// Compute a single entry's weighting factor using integer-only arithmetic.
///
/// `weight = stake × (1 + ln_milli(1 + age_years_milli) / 1000)`
///
/// age_years is passed as milli-years (× 1000) from the caller to avoid
/// floating point. For example, 1 year = 1000, 6 months = 500.
///
/// The logarithmic seniority bonus means older entries earn proportionally
/// more per-coin, but the marginal gain diminishes over time, preventing
/// permanent dominance by early stakers.
pub fn compute_refiner_weight(stake: FlakeAmount, age_years_milli: u64) -> FlakeAmount {
    // 1 + age in milli
    let one_plus_age_milli = 1000u64.saturating_add(age_years_milli);
    // ln(1 + age) in milli
    let ln_bonus = ln_milli(one_plus_age_milli);
    // total multiplier in milli: 1000 + ln_bonus
    let multiplier_milli = 1000u64.saturating_add(ln_bonus);
    // weight = stake × multiplier / 1000
    ((stake as u128 * multiplier_milli as u128) / 1000) as FlakeAmount
}

/// Compute stake coverage — the ratio of total bonded $OPL to total issued
/// $OPL, clamped to [0.0, 1.0]. This single metric determines how block
/// rewards are split between miners and refiners.
pub fn compute_stake_coverage(total_bonded: FlakeAmount, total_issued: FlakeAmount) -> f64 {
    if total_issued == 0 {
        return 0.0;
    }
    (total_bonded as f64 / total_issued as f64).min(1.0)
}

/// Compute the suggested fee for the next block using an EMA of the
/// previous block's transaction fees.
///
/// The suggested fee starts at MIN_FEE (1 Flake) and adjusts via exponential
/// moving average with a smoothing factor of 0.1. This provides a market-
/// driven fee signal without governance — fees are purely between transactors
/// and block producers.
pub fn compute_suggested_fee(previous_block_fees: FlakeAmount, previous_suggested_fee: FlakeAmount) -> FlakeAmount {
    // EMA with α = 0.1: new = α × current + (1 - α) × old
    // In integer arithmetic: new = (current + 9 × old) / 10
    let current = previous_block_fees;
    let old = previous_suggested_fee;
    let ema = (current.saturating_add(old.saturating_mul(9))) / 10;
    ema.max(1) // Floor at MIN_FEE (1 Flake)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::BASE_REWARD;

    // ─── difficulty_to_target tests ───

    #[test]
    fn difficulty_to_target_at_1() {
        // Difficulty 1 means 1 leading zero bit. Target = 2^63 - 1.
        assert_eq!(difficulty_to_target(1), (1u128 << 63) as u64 - 1);
    }

    #[test]
    fn difficulty_to_target_at_10() {
        // Difficulty 10 means 10 leading zero bits. Target = 2^54 - 1.
        assert_eq!(difficulty_to_target(10), (1u128 << 54) as u64 - 1);
    }

    #[test]
    fn difficulty_to_target_at_20() {
        // Difficulty 20 means 20 leading zero bits. Target = 2^44 - 1.
        assert_eq!(difficulty_to_target(20), (1u128 << 44) as u64 - 1);
    }

    #[test]
    fn difficulty_to_target_at_64() {
        // Difficulty 64 means all bits must be zero. Target = 0 (only hash 0 passes).
        assert_eq!(difficulty_to_target(64), 0);
    }

    #[test]
    fn difficulty_to_target_zero_returns_0() {
        assert_eq!(difficulty_to_target(0), 0);
    }

    #[test]
    fn difficulty_to_target_above_64_returns_0() {
        // Difficulty > 64 is impossible for a u64 — no valid hash possible
        assert_eq!(difficulty_to_target(65), 0);
        assert_eq!(difficulty_to_target(128), 0);
    }

    #[test]
    fn difficulty_to_target_monotonically_decreasing() {
        // Higher difficulty should have lower or equal target
        let mut prev = difficulty_to_target(1);
        for d in 2..=64 {
            let curr = difficulty_to_target(d);
            assert!(curr <= prev, "target({}) = {} > target({}) = {}", d, curr, d - 1, prev);
            prev = curr;
        }
    }

    // ─── Vein yield tests ───

    #[test]
    fn vein_yield_at_min_difficulty() {
        // At difficulty 1, target = 2^63 - 1. A very low hash gives high yield.
        let yield_val = compute_vein_yield(1, 1);
        assert!(yield_val > 1000, "yield should exceed 1.0x for a very good hash");
    }

    #[test]
    fn vein_yield_minimum_for_valid_pow() {
        // At difficulty 10, target = 2^54 - 1.
        // A hash that is much less than the target gives a bonus.
        let target_10 = difficulty_to_target(10);
        let yield_val = compute_vein_yield(10, target_10 / 20);
        assert!(yield_val >= 1000);
    }

    #[test]
    fn vein_yield_at_target_boundary() {
        // A hash exactly at target should give minimum yield (no bonus)
        let target_10 = difficulty_to_target(10);
        let yield_val = compute_vein_yield(10, target_10);
        assert_eq!(yield_val, 1000, "hash at target should give minimum yield");
    }

    #[test]
    fn vein_yield_above_target_gives_minimum() {
        // A hash above target is invalid for PoW, but vein yield returns minimum
        let yield_val = compute_vein_yield(10, u64::MAX);
        assert_eq!(yield_val, 1000);
    }

    #[test]
    fn vein_yield_zero_hash_returns_minimum() {
        let yield_val = compute_vein_yield(10, 0);
        assert_eq!(yield_val, 1000);
    }

    #[test]
    fn vein_yield_zero_difficulty_returns_minimum() {
        let yield_val = compute_vein_yield(0, 100);
        assert_eq!(yield_val, 1000);
    }

    #[test]
    fn block_reward_with_vein_yield() {
        // At difficulty 1 with a very good hash, reward should exceed base
        let base = compute_base_reward(BASE_REWARD, 1);
        let with_yield = compute_block_reward(BASE_REWARD, 1, 1);
        assert!(with_yield >= base);
    }

    #[test]
    fn base_reward_at_min_difficulty() {
        let reward = compute_base_reward(BASE_REWARD, 1);
        assert_eq!(reward, BASE_REWARD);
    }

    #[test]
    fn reward_decreases_with_difficulty() {
        let r1 = compute_base_reward(BASE_REWARD, 1);
        let r10 = compute_base_reward(BASE_REWARD, 10);
        let r100 = compute_base_reward(BASE_REWARD, 100);
        assert!(r1 > r10);
        assert!(r10 > r100);
    }

    #[test]
    fn ln_milli_known_values() {
        // ln(1.0) ≈ 0
        assert!(ln_milli(1000) < 5);
        // ln(2.0) ≈ 693 milli (i.e., 0.693)
        let ln2 = ln_milli(2000);
        assert!(ln2 > 680 && ln2 < 710, "ln(2) = {} milli, expected ~693", ln2);
        // ln(10) ≈ 2303 milli
        let ln10 = ln_milli(10000);
        assert!(ln10 > 2280 && ln10 < 2330, "ln(10) = {} milli, expected ~2303", ln10);
    }

    #[test]
    fn ln_milli_below_threshold() {
        assert_eq!(ln_milli(999), 0);
        assert_eq!(ln_milli(0), 0);
    }

    #[test]
    fn refiner_weight_increases_with_age() {
        let w1 = compute_refiner_weight(100_000, 500);
        let w2 = compute_refiner_weight(100_000, 2000);
        let w5 = compute_refiner_weight(100_000, 5000);
        assert!(w1 < w2);
        assert!(w2 < w5);
    }

    #[test]
    fn refiner_weight_zero_age() {
        let w = compute_refiner_weight(100_000, 0);
        assert_eq!(w, 100_000);
    }

    #[test]
    fn stake_coverage_calculation() {
        assert_eq!(compute_stake_coverage(500, 1000), 0.5);
        assert_eq!(compute_stake_coverage(0, 1000), 0.0);
        assert_eq!(compute_stake_coverage(1000, 1000), 1.0);
        assert!(compute_stake_coverage(2000, 1000) <= 1.0);
    }

    #[test]
    fn suggested_fee_starts_at_minimum() {
        let fee = compute_suggested_fee(0, 0);
        assert_eq!(fee, 1);
    }

    #[test]
    fn suggested_fee_updates_via_ema() {
        let fee = compute_suggested_fee(10_000, 1_000);
        assert_eq!(fee, 1900);
    }

    #[test]
    fn suggested_fee_floors_at_min_fee() {
        let fee = compute_suggested_fee(0, 0);
        assert_eq!(fee, 1);
    }

    #[test]
    fn base_reward_is_332_opl() {
        assert_eq!(BASE_REWARD, 332 * opolys_core::FLAKES_PER_OPL);
    }

    #[test]
    fn refiner_reward_proportional_to_weight() {
        let reward = 1_000_000u64;
        let r1 = compute_refiner_reward(reward, 100_000, 1000, 200_000);
        assert!(r1 > 0);
        assert!(r1 < reward);
    }

    /// Verify that vein yield uses the leading-zero-bits difficulty model
    /// correctly by comparing against manually computed targets.
    /// With sqrt(ln) compression: yield = 1 + sqrt(ln(target/hash)) in milli.
    #[test]
    fn vein_yield_uses_leading_zero_bits_difficulty() {
        // At difficulty 10, target = 2^54 - 1
        // A hash of 1 gives: sqrt(ln(target/1)) * 1000 ≈ sqrt(37.5) * 1000 ≈ 6123
        let yield_val = compute_vein_yield(10, 1);
        assert!(yield_val > 5000, "difficulty 10 with hash 1 should give significant yield, got {}", yield_val);

        // At difficulty 1, target = 2^63 - 1
        // A hash of 1 gives: sqrt(ln(2^63 - 1)) ≈ sqrt(43.7) ≈ 6.61
        let yield_val = compute_vein_yield(1, 1);
        assert!(yield_val > 5500, "difficulty 1 with hash 1 should give significant yield, got {}", yield_val);
    }
}