use opolys_core::{BASE_REWARD, FleckAmount, BlockHeight, MIN_DIFFICULTY};

pub fn compute_block_reward(difficulty: u64, discovery_bonus: u64) -> FleckAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    BASE_REWARD / effective_difficulty * discovery_bonus
}

pub fn compute_base_reward(difficulty: u64) -> FleckAmount {
    let effective_difficulty = difficulty.max(MIN_DIFFICULTY);
    BASE_REWARD / effective_difficulty
}

pub fn compute_miner_reward(difficulty: u64, discovery_bonus: u64) -> FleckAmount {
    compute_block_reward(difficulty, discovery_bonus)
}

pub fn compute_validator_reward(
    block_reward: FleckAmount,
    validator_stake: FleckAmount,
    validator_age_years: f64,
    total_weight: FleckAmount,
) -> FleckAmount {
    if total_weight == 0 {
        return 0;
    }
    let weight = compute_validator_weight(validator_stake, validator_age_years);
    ((block_reward as u128 * weight as u128) / total_weight as u128) as FleckAmount
}

pub fn compute_validator_weight(stake: FleckAmount, age_years: f64) -> FleckAmount {
    let multiplier = 1.0_f64 + (1.0_f64 + age_years).ln();
    (stake as f64 * multiplier) as FleckAmount
}

pub fn compute_pow_share(stake_coverage: f64) -> f64 {
    1.0 - stake_coverage
}

pub fn compute_pos_share(stake_coverage: f64) -> f64 {
    stake_coverage
}

pub fn compute_stake_coverage(total_bonded: FleckAmount, total_issued: FleckAmount) -> f64 {
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
}