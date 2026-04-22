use opolys_core::{BASE_REWARD, MIN_DIFFICULTY, RETARGET_EPOCH, BlockHeight};

#[derive(Debug, Clone)]
pub struct DifficultyTarget {
    pub target: u64,
    pub retarget: u64,
    pub consensus_floor: u64,
}

impl DifficultyTarget {
    pub fn effective_difficulty(&self) -> u64 {
        self.retarget.max(self.consensus_floor).max(MIN_DIFFICULTY)
    }
}

pub fn compute_consensus_floor(total_issued: u64, bonded_stake: u64) -> u64 {
    if bonded_stake == 0 {
        return 0;
    }
    total_issued / bonded_stake
}

pub fn compute_next_difficulty(
    current_difficulty: u64,
    current_height: BlockHeight,
    block_timestamps: &[u64],
    total_issued: u64,
    bonded_stake: u64,
) -> DifficultyTarget {
    let retarget = compute_retarget(current_difficulty, current_height, block_timestamps);
    let consensus_floor = compute_consensus_floor(total_issued, bonded_stake);

    DifficultyTarget {
        target: compute_target(retarget.max(consensus_floor).max(MIN_DIFFICULTY)),
        retarget,
        consensus_floor,
    }
}

fn compute_retarget(current_difficulty: u64, current_height: BlockHeight, block_timestamps: &[u64]) -> u64 {
    if current_height < RETARGET_EPOCH {
        return current_difficulty.max(MIN_DIFFICULTY);
    }

    let epoch_start = current_height.saturating_sub(RETARGET_EPOCH);
    if epoch_start as usize >= block_timestamps.len() {
        return current_difficulty.max(MIN_DIFFICULTY);
    }

    let start_idx = epoch_start as usize;
    let end_idx = if (current_height as usize) < block_timestamps.len() {
        current_height as usize
    } else {
        block_timestamps.len() - 1
    };

    if end_idx <= start_idx {
        return current_difficulty.max(MIN_DIFFICULTY);
    }

    let actual_time = block_timestamps[end_idx].saturating_sub(block_timestamps[start_idx]);
    let expected_time = RETARGET_EPOCH * opolys_core::BLOCK_TARGET_TIME_SECS;

    if actual_time == 0 {
        return current_difficulty.saturating_mul(4);
    }

    let numerator = current_difficulty as u128 * actual_time as u128;
    let denominator = expected_time as u128;
    let new_difficulty = (numerator / denominator) as u64;

    let max_new = current_difficulty.saturating_mul(4);
    let min_new = current_difficulty / 4;

    new_difficulty.clamp(min_new, max_new).max(MIN_DIFFICULTY)
}

fn compute_target(difficulty: u64) -> u64 {
    if difficulty == 0 {
        return u64::MAX;
    }
    u64::MAX / difficulty
}

pub fn check_proof_of_work(hash_value: u64, difficulty: u64) -> bool {
    let target = compute_target(difficulty);
    hash_value < target
}

pub fn compute_discovery_bonus(difficulty: u64, hash_value: u64) -> u64 {
    if difficulty == 0 || hash_value == 0 {
        return 1;
    }
    let ratio = u128::from(u64::MAX) / (u128::from(difficulty) * u128::from(hash_value));
    if ratio < 1 {
        return 1;
    }
    let sqrt_ratio = (ratio as f64).sqrt() as u64;
    sqrt_ratio.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_floor_zero_bonded() {
        assert_eq!(compute_consensus_floor(1_000_000, 0), 0);
    }

    #[test]
    fn consensus_floor_balanced() {
        let floor = compute_consensus_floor(1_000_000, 1_000_000);
        assert_eq!(floor, 1);
    }

    #[test]
    fn consensus_floor_more_issued_than_stake() {
        let floor = compute_consensus_floor(10_000_000, 1_000_000);
        assert_eq!(floor, 10);
    }

    #[test]
    fn effective_difficulty_max() {
        let dt = DifficultyTarget {
            target: 100,
            retarget: 50,
            consensus_floor: 200,
        };
        assert_eq!(dt.effective_difficulty(), 200);
    }

    #[test]
    fn discovery_bonus_minimum() {
        let bonus = compute_discovery_bonus(1, u64::MAX);
        assert_eq!(bonus, 1);
    }

    #[test]
    fn discovery_bonus_large() {
        let bonus = compute_discovery_bonus(1, 1);
        assert!(bonus > 1);
    }

    #[test]
    fn check_pow_easy_difficulty() {
        assert!(check_proof_of_work(0, 1));
        assert!(!check_proof_of_work(u64::MAX, 1));
    }

    #[test]
    fn retarget_at_epoch_boundary() {
        let timestamps: Vec<u64> = (0..=2000).map(|i| i * 120).collect();
        let new_diff = compute_retarget(100, 1000, &timestamps);
        assert!(new_diff >= MIN_DIFFICULTY);
    }

    #[test]
    fn compute_next_difficulty_integrates_consensus_floor() {
        let timestamps: Vec<u64> = (0..=2000).map(|i| i * 120).collect();
        let result = compute_next_difficulty(100, 1000, &timestamps, 10_000_000, 1_000_000);
        assert!(result.effective_difficulty() >= 10);
    }
}