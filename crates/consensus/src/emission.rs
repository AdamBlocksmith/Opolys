//! # Block reward and emission schedule for Opolys.
//!
//! Opolys has **no fixed emission schedule** and no halvings. Instead, block
//! rewards emerge from chain state:
//!
//! - **Vein yield** = `1 + sqrt(ln(target / hash_int))`, where
//!   `target = 2^(64-D) - 1` and D is the EVO-OMAP difficulty (leading zero
//!   bits). This uses fixed-point integer math only, so every platform computes
//!   the same consensus reward.
//! - **Base reward** = `BASE_REWARD / effective_difficulty`. As difficulty
//!   rises, the per-block reward naturally declines — mimicking the
//!   diminishing returns of real-world gold extraction.
//! - **Refiner weight** = bonded stake. Bond age is tracked for provenance and
//!   FIFO unbonding, but it does not increase producer-selection or finality
//!   weight.
//! - **Stake coverage** — the ratio of bonded stake to total issued supply —
//!   is an observability and difficulty-floor signal, not a passive reward
//!   splitter.
//!
//! There is no governance body and no parameter votes. Fee markets and chain
//! state drive everything.

use opolys_core::{CAPACITY_RATIO, FlakeAmount, MIN_DIFFICULTY};

const Q32_ONE: u128 = 1u128 << 32;
const LN_2_Q32: u128 = 2_977_044_471;

fn saturating_u128_to_flakes(value: u128) -> FlakeAmount {
    value.min(FlakeAmount::MAX as u128) as FlakeAmount
}

fn integer_sqrt_floor(n: u128) -> u128 {
    if n < 2 {
        return n;
    }

    let bit_len = 128 - n.leading_zeros() as u128;
    let mut left = 1u128;
    let mut right = 1u128 << bit_len.div_ceil(2);
    let mut result = 1u128;

    while left <= right {
        let mid = (left + right) / 2;
        let square = mid.saturating_mul(mid);
        if square <= n {
            result = mid;
            left = mid + 1;
        } else {
            right = mid - 1;
        }
    }

    result
}

fn integer_sqrt_round(n: u128) -> u128 {
    let floor = integer_sqrt_floor(n);
    let floor_square = floor.saturating_mul(floor);
    let next = floor.saturating_add(1);
    let next_square = next.saturating_mul(next);
    if next_square.saturating_sub(n) < n.saturating_sub(floor_square) {
        next
    } else {
        floor
    }
}

fn ln_mantissa_q32(mantissa_q32: u128) -> u128 {
    if mantissa_q32 <= Q32_ONE {
        return 0;
    }

    let numerator = (mantissa_q32 - Q32_ONE) << 32;
    let denominator = mantissa_q32 + Q32_ONE;
    let y_q32 = numerator / denominator;
    let y2_q32 = (y_q32 * y_q32) >> 32;
    let mut term_q32 = y_q32;
    let mut sum_q32 = 0u128;

    for divisor in (1u128..=39).step_by(2) {
        sum_q32 = sum_q32.saturating_add(term_q32 / divisor);
        term_q32 = (term_q32 * y2_q32) >> 32;
        if term_q32 == 0 {
            break;
        }
    }

    sum_q32.saturating_mul(2)
}

fn ln_u64_q32(value: u64) -> u128 {
    if value <= 1 {
        return 0;
    }

    let log2_floor = 63 - value.leading_zeros() as u64;
    let mantissa_q32 = if log2_floor <= 32 {
        (value as u128) << (32 - log2_floor)
    } else {
        (value as u128) >> (log2_floor - 32)
    };

    (log2_floor as u128)
        .saturating_mul(LN_2_Q32)
        .saturating_add(ln_mantissa_q32(mantissa_q32))
}

fn q32_to_milli(q32: u128) -> u64 {
    ((q32.saturating_mul(1000).saturating_add(Q32_ONE / 2)) / Q32_ONE).min(u64::MAX as u128) as u64
}

fn ln_ratio_milli(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 || numerator <= denominator {
        return 0;
    }
    q32_to_milli(ln_u64_q32(numerator).saturating_sub(ln_u64_q32(denominator)))
}

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
pub fn compute_block_reward(
    base_reward: FlakeAmount,
    difficulty: u64,
    pow_hash_value: u64,
) -> FlakeAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    let base = base_reward / effective_difficulty;
    let yield_milli = compute_vein_yield(difficulty, pow_hash_value);
    // yield_milli is in thousandths (milli), so divide by 1000.
    saturating_u128_to_flakes((base as u128 * yield_milli as u128) / 1000)
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
/// The result is clamped to a minimum of 1000 (1.0x) — every valid block
/// earns at least the base reward.
pub fn compute_vein_yield(difficulty: u64, hash_int: u64) -> u64 {
    // hash_int == 0 returns the floor and carries no ore-discovery bonus.
    // Proof-of-Refinement blocks receive no issuance reward at the node layer.
    // Miners earn variable income based on hash luck (1x to ~4x).
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
    // Compute sqrt(ln(target / hash_int)) using integer milli-units.
    let ln_ratio_milli = ln_ratio_milli(target, hash_int);
    let ln_val = integer_sqrt_round(ln_ratio_milli as u128 * 1000);
    // vein_yield = 1 + sqrt(ln(ratio)), in milli
    1000u64.saturating_add(ln_val.min(u64::MAX as u128) as u64)
}

/// Natural log computation returning ln(x / 1000) × 1000, where x is in
/// milli units (1000 = 1.0, 2000 = 2.0, etc.).
///
/// For x < 1000 (sub-unity values), returns 0.
/// For x >= 1000, the result is computed with deterministic fixed-point
/// integer arithmetic.
pub fn ln_milli(x: u64) -> u64 {
    if x < 1000 {
        return 0;
    }
    ln_ratio_milli(x, 1000)
}

/// Compute a single entry's weighting factor.
///
/// Opolys intentionally gives no age-based yield. A bonded entry's weight
/// is exactly its stake, so rewards and finality come from posted collateral,
/// not time-based accrual.
pub fn compute_refiner_weight(stake: FlakeAmount, _age_years_milli: u64) -> FlakeAmount {
    stake
}

/// Compute stake coverage — the ratio of total bonded $OPL to total issued
/// Returns milli-units clamped to [0, 1000], keeping the consensus-facing
/// calculation integer-only.
pub fn compute_stake_coverage(total_bonded: FlakeAmount, total_issued: FlakeAmount) -> u64 {
    if total_issued == 0 {
        return 0;
    }
    ((total_bonded as u128 * 1000) / total_issued as u128).min(1000) as u64
}

/// Compute the suggested fee for the next block using an EMA of the previous
/// block's average explicit fee from successful transactions.
///
/// The suggested fee starts at MIN_FEE (1 Flake) and adjusts via exponential
/// moving average with a window derived from `CAPACITY_RATIO`. This provides a market-
/// driven fee signal without governance — fees are purely between transactors
/// and block producers.
pub fn compute_suggested_fee(
    previous_block_fees: FlakeAmount,
    successful_transaction_count: u64,
    previous_suggested_fee: FlakeAmount,
) -> FlakeAmount {
    // EMA with α = 0.1: new = α × current + (1 - α) × old
    // In integer arithmetic: new = (current + 9 × old) / 10
    let current = previous_block_fees
        .checked_div(successful_transaction_count)
        .unwrap_or(opolys_core::MIN_FEE);
    let old = previous_suggested_fee;
    // Weighting is derived from chain capacity: one block of new demand plus
    // the remaining mempool/block capacity window of prior fee pressure.
    let window = CAPACITY_RATIO.max(1);
    let old_weight = window.saturating_sub(1);
    let ema = (current.saturating_add(old.saturating_mul(old_weight))) / window;
    ema.max(opolys_core::MIN_FEE)
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
            assert!(
                curr <= prev,
                "target({}) = {} > target({}) = {}",
                d,
                curr,
                d - 1,
                prev
            );
            prev = curr;
        }
    }

    // ─── Vein yield tests ───

    #[test]
    fn vein_yield_at_min_difficulty() {
        // At difficulty 1, target = 2^63 - 1. A very low hash gives high yield.
        let yield_val = compute_vein_yield(1, 1);
        assert!(
            yield_val > 1000,
            "yield should exceed 1.0x for a very good hash"
        );
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
    fn reward_math_saturates_instead_of_truncating() {
        assert_eq!(compute_block_reward(u64::MAX, 1, 1), u64::MAX);
        assert_eq!(compute_refiner_weight(u64::MAX, u64::MAX), u64::MAX);
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
        assert!(
            ln2 > 680 && ln2 < 710,
            "ln(2) = {} milli, expected ~693",
            ln2
        );
        // ln(10) ≈ 2303 milli
        let ln10 = ln_milli(10000);
        assert!(
            ln10 > 2280 && ln10 < 2330,
            "ln(10) = {} milli, expected ~2303",
            ln10
        );
    }

    #[test]
    fn ln_milli_below_threshold() {
        assert_eq!(ln_milli(999), 0);
        assert_eq!(ln_milli(0), 0);
    }

    #[test]
    fn refiner_weight_ignores_age() {
        let w1 = compute_refiner_weight(100_000, 500);
        let w2 = compute_refiner_weight(100_000, 2000);
        let w5 = compute_refiner_weight(100_000, 5000);
        assert_eq!(w1, 100_000);
        assert_eq!(w2, 100_000);
        assert_eq!(w5, 100_000);
    }

    #[test]
    fn refiner_weight_zero_age() {
        let w = compute_refiner_weight(100_000, 0);
        assert_eq!(w, 100_000);
    }

    #[test]
    fn stake_coverage_calculation() {
        assert_eq!(compute_stake_coverage(500, 1000), 500);
        assert_eq!(compute_stake_coverage(0, 1000), 0);
        assert_eq!(compute_stake_coverage(1000, 1000), 1000);
        assert_eq!(compute_stake_coverage(2000, 1000), 1000);
    }

    #[test]
    fn suggested_fee_starts_at_minimum() {
        let fee = compute_suggested_fee(0, 0, 0);
        assert_eq!(fee, 1);
    }

    #[test]
    fn suggested_fee_updates_via_ema() {
        let fee = compute_suggested_fee(10_000, 1, 1_000);
        assert_eq!(fee, 1900);
    }

    #[test]
    fn suggested_fee_uses_average_burned_fee_not_total_block_fees() {
        let fee = compute_suggested_fee(100_000, 100, 1_000);
        assert_eq!(fee, 1_000);
    }

    #[test]
    fn suggested_fee_empty_block_drifts_toward_min_fee() {
        let fee = compute_suggested_fee(0, 0, 1_000);
        assert_eq!(fee, 900);
    }

    #[test]
    fn suggested_fee_floors_at_min_fee() {
        let fee = compute_suggested_fee(0, 0, 0);
        assert_eq!(fee, 1);
    }

    #[test]
    fn base_reward_is_332_opl() {
        assert_eq!(BASE_REWARD, 332 * opolys_core::FLAKES_PER_OPL);
    }

    /// Verify that vein yield uses the leading-zero-bits difficulty model
    /// correctly by comparing against manually computed targets.
    /// With sqrt(ln) compression: yield = 1 + sqrt(ln(target/hash)) in milli.
    #[test]
    fn vein_yield_uses_leading_zero_bits_difficulty() {
        // At difficulty 10, target = 2^54 - 1
        // A hash of 1 gives: sqrt(ln(target/1)) * 1000 ≈ sqrt(37.5) * 1000 ≈ 6123
        let yield_val = compute_vein_yield(10, 1);
        assert!(
            yield_val > 5000,
            "difficulty 10 with hash 1 should give significant yield, got {}",
            yield_val
        );

        // At difficulty 1, target = 2^63 - 1
        // A hash of 1 gives: sqrt(ln(2^63 - 1)) ≈ sqrt(43.7) ≈ 6.61
        let yield_val = compute_vein_yield(1, 1);
        assert!(
            yield_val > 5500,
            "difficulty 1 with hash 1 should give significant yield, got {}",
            yield_val
        );
    }
}
