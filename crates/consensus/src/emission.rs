//! # Block reward and emission schedule for Opolys.
//!
//! Opolys has **no fixed emission schedule** and no halvings. Instead, block
//! rewards emerge from chain state:
//!
//! - **Base reward** = `BASE_REWARD / effective_difficulty`. As difficulty
//!   rises, the per-block reward naturally declines — mimicking the
//!   diminishing returns of real-world gold extraction.
//! - **Discovery bonus** amplifies the reward when a miner finds an
//!   exceptionally good hash (far below target), rewarding luck and effort.
//! - **Validator weight** = `stake × (1 + ln(1 + age_years))`, giving a
//!   logarithmic seniority bonus rather than linear stake dominance.
//! - **Stake coverage** — the ratio of bonded stake to total issued supply —
//!   determines how much of each block reward flows to miners vs. validators.
//!   At 0% coverage, all rewards go to miners; at 100%, all go to validators.
//!
//! There is no governance body and no parameter votes. Fee markets and chain
//! state drive everything.

use opolys_core::{BASE_REWARD, FlakeAmount, MIN_DIFFICULTY};

/// Compute the total block reward for a given difficulty and discovery bonus.
///
/// The formula is: `(BASE_REWARD / effective_difficulty) * discovery_bonus`.
///
/// Effective difficulty is floored at `MIN_DIFFICULTY`, so early blocks still
/// yield meaningful rewards. The discovery bonus is typically 1 (no bonus)
/// but can be higher for exceptionally good PoW hashes.
pub fn compute_block_reward(difficulty: u64, discovery_bonus: u64) -> FlakeAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    BASE_REWARD / effective_difficulty * discovery_bonus
}

/// Compute the base block reward without any discovery bonus.
///
/// This is the baseline reward every block earns. It shrinks as the network's
/// effective difficulty grows, following the same economic logic as gold:
/// harder extraction → smaller yield.
pub fn compute_base_reward(difficulty: u64) -> FlakeAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    BASE_REWARD / effective_difficulty
}

/// Compute the miner's share of the block reward.
///
/// In Opolys, miners receive the full block reward (base × discovery bonus)
/// when they produce a PoW block. There is no separate miner share formula —
/// this function is an alias of `compute_block_reward` for clarity.
pub fn compute_miner_reward(difficulty: u64, discovery_bonus: u64) -> FlakeAmount {
    compute_block_reward(difficulty, discovery_bonus)
}

/// Compute a validator's share of the block reward based on their weighted
/// stake relative to the total weight of all active validators.
///
/// Weight is `stake × (1 + ln(1 + age_years))`, giving a logarithmic
/// seniority bonus. Rewards are distributed proportionally to weight.
pub fn compute_validator_reward(
    block_reward: FlakeAmount,
    validator_stake: FlakeAmount,
    validator_age_years: f64,
    total_weight: FlakeAmount,
) -> FlakeAmount {
    if total_weight == 0 {
        return 0;
    }
    let weight = compute_validator_weight(validator_stake, validator_age_years);
    // u128 intermediate prevents overflow on large reward × weight products.
    ((block_reward as u128 * weight as u128) / total_weight as u128) as FlakeAmount
}

/// Compute a validator's weighting factor: `stake × (1 + ln(1 + age_years))`.
///
/// This gives a logarithmic — not linear — seniority bonus. Validators who
/// stay bonded longer earn proportionally more, but the marginal gain
/// diminishes over time, preventing permanent dominance by early stakers.
pub fn compute_validator_weight(stake: FlakeAmount, age_years: f64) -> FlakeAmount {
    let multiplier = 1.0_f64 + (1.0_f64 + age_years).ln();
    (stake as f64 * multiplier) as FlakeAmount
}

/// Compute the PoW (miner) share of block rewards as a fraction of total
/// rewards. Equals `1.0 - stake_coverage`, so as more $OPL is bonded,
/// miners receive a smaller share.
pub fn compute_pow_share(stake_coverage: f64) -> f64 {
    1.0 - stake_coverage
}

/// Compute the PoS (validator) share of block rewards as a fraction of total
/// rewards. Equals `stake_coverage`, so as more $OPL is bonded, validators
/// receive a larger share.
pub fn compute_pos_share(stake_coverage: f64) -> f64 {
    stake_coverage
}

/// Compute stake coverage — the ratio of total bonded $OPL to total issued
/// $OPL, clamped to [0.0, 1.0]. This single metric determines how block
/// rewards are split between miners and validators.
pub fn compute_stake_coverage(total_bonded: FlakeAmount, total_issued: FlakeAmount) -> f64 {
    if total_issued == 0 {
        return 0.0;
    }
    (total_bonded as f64 / total_issued as f64).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_reward_at_min_difficulty() {
        let reward = compute_base_reward(1);
        assert_eq!(reward, BASE_REWARD);
    }

    #[test]
    fn reward_decreases_with_difficulty() {
        let r1 = compute_base_reward(1);
        let r10 = compute_base_reward(10);
        let r100 = compute_base_reward(100);
        assert!(r1 > r10);
        assert!(r10 > r100);
    }

    #[test]
    fn discovery_bonus_increases_reward() {
        let base = compute_base_reward(10);
        let with_bonus = compute_block_reward(10, 5);
        assert_eq!(with_bonus, base * 5);
    }

    #[test]
    fn validator_reward_proportional_to_weight() {
        let reward = 1_000_000u64;
        let r1 = compute_validator_reward(reward, 100_000, 1.0, 200_000);
        assert!(r1 > 0);
        assert!(r1 < reward);
    }

    #[test]
    fn validator_weight_increases_with_age() {
        let w1 = compute_validator_weight(100_000, 0.5);
        let w2 = compute_validator_weight(100_000, 2.0);
        let w5 = compute_validator_weight(100_000, 5.0);
        assert!(w1 < w2);
        assert!(w2 < w5);
    }

    #[test]
    fn stake_coverage_calculation() {
        assert_eq!(compute_stake_coverage(500, 1000), 0.5);
        assert_eq!(compute_stake_coverage(0, 1000), 0.0);
        assert_eq!(compute_stake_coverage(1000, 1000), 1.0);
        assert!(compute_stake_coverage(2000, 1000) <= 1.0);
    }

    #[test]
    fn pow_pos_transition_continuous() {
        let coverage = 0.3;
        assert!((compute_pow_share(coverage) - 0.7).abs() < 0.001);
        assert!((compute_pos_share(coverage) - 0.3).abs() < 0.001);
    }

    #[test]
    fn miner_reward_equals_block_reward() {
        let miner_reward = compute_miner_reward(10, 3);
        let block_reward = compute_block_reward(10, 3);
        assert_eq!(miner_reward, block_reward);
    }

    #[test]
    fn base_reward_is_440_opl() {
        assert_eq!(BASE_REWARD, 440 * opolys_core::FLAKES_PER_OPL);
    }
}