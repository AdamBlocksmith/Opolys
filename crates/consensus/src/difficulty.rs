//! # Adaptive difficulty retargeting and proof-of-work verification.
//!
//! Opolys has **no hard cap** on supply and no hardcoded difficulty schedules.
//! Instead, difficulty emerges from two components:
//!
//! 1. **Retarget** — a Bitcoin-style adjustment every `RETARGET_EPOCH` blocks
//!    that compares actual block times against the target interval.
//! 2. **Consensus floor** — `total_issued / bonded_stake`, a minimum difficulty
//!    that rises as more $OPL enters circulation relative to staked supply.
//!
//! The effective difficulty is the maximum of the retarget, the consensus floor,
//! and `MIN_DIFFICULTY`. This dual mechanism ensures that difficulty grows
//! organically as the network matures and staking participation evolves.

use opolys_core::{MIN_DIFFICULTY, RETARGET_EPOCH, BlockHeight};

/// The computed difficulty target for a given block.
///
/// Contains the retarget value, consensus floor, and the combined effective
/// difficulty (the maximum of retarget, consensus floor, and `MIN_DIFFICULTY`).
/// The `target` field is the numerical hash threshold for PoW validation
/// (`u64::MAX / effective_difficulty`).
#[derive(Debug, Clone)]
pub struct DifficultyTarget {
    /// Numerical hash target that a valid PoW must be below.
    pub target: u64,
    /// Difficulty computed by the retarget algorithm alone.
    pub retarget: u64,
    /// Difficulty floor derived from `total_issued / bonded_stake`.
    pub consensus_floor: u64,
}

impl DifficultyTarget {
    /// Returns the effective difficulty: the maximum of retarget, consensus
    /// floor, and the global `MIN_DIFFICULTY`. This is the value used for
    /// PoW validation and reward computation.
    pub fn effective_difficulty(&self) -> u64 {
        self.retarget.max(self.consensus_floor).max(MIN_DIFFICULTY)
    }
}

/// Compute the consensus floor difficulty from circulating supply and bonded stake.
///
/// The floor rises as more $OPL is issued relative to bonded stake, ensuring
/// that PoW difficulty cannot fall below the organic growth rate of the network.
/// When `bonded_stake` is zero, the floor is zero (no validators yet).
pub fn compute_consensus_floor(total_issued: u64, bonded_stake: u64) -> u64 {
    if bonded_stake == 0 {
        return 0;
    }
    total_issued / bonded_stake
}

/// Compute the next block's difficulty target by combining retarget and
/// consensus floor. The effective difficulty is the maximum of the two
/// and `MIN_DIFFICULTY`, guaranteeing a baseline difficulty even early on.
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

/// Retarget difficulty every `RETARGET_EPOCH` blocks by comparing actual
/// block times to the expected target interval.
///
/// If the epoch was too fast, difficulty increases; if too slow, it decreases.
/// Adjustments are clamped to [current/4, current*4] to prevent wild swings,
/// and never fall below `MIN_DIFFICULTY`.
fn compute_retarget(current_difficulty: u64, current_height: BlockHeight, block_timestamps: &[u64]) -> u64 {
    // Not enough blocks for a retarget epoch yet — hold at current difficulty.
    if current_height < RETARGET_EPOCH {
        return current_difficulty.max(MIN_DIFFICULTY);
    }

    let epoch_start = current_height.saturating_sub(RETARGET_EPOCH);
    if epoch_start as usize >= block_timestamps.len() {
        return current_difficulty.max(MIN_DIFFICULTY);
    }

    let start_idx = epoch_start as usize;
    // Clamp end_idx to the available timestamp array to avoid panics.
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

    // If timestamps are degenerate (zero elapsed time), spike difficulty 4x.
    if actual_time == 0 {
        return current_difficulty.saturating_mul(4);
    }

    // Standard retarget: scale difficulty proportionally to expected vs actual time.
    // Uses u128 intermediate to prevent overflow on large difficulty values.
    let numerator = current_difficulty as u128 * actual_time as u128;
    let denominator = expected_time as u128;
    let new_difficulty = (numerator / denominator) as u64;

    // Clamp to [current/4, current*4] to prevent violent swings per epoch.
    let max_new = current_difficulty.saturating_mul(4);
    let min_new = current_difficulty / 4;

    new_difficulty.clamp(min_new, max_new).max(MIN_DIFFICULTY)
}

/// Convert difficulty to a numerical hash target: `u64::MAX / difficulty`.
///
/// A valid PoW hash must be strictly less than this target. Higher difficulty
/// means a lower target and harder mining. Returns `u64::MAX` for difficulty
/// zero (trivially satisfied).
fn compute_target(difficulty: u64) -> u64 {
    if difficulty == 0 {
        return u64::MAX;
    }
    u64::MAX / difficulty
}

/// Check whether a hash value satisfies the difficulty requirement.
///
/// Returns `true` if `hash_value < target(difficulty)`, meaning the PoW is
/// valid for the given difficulty level.
pub fn check_proof_of_work(hash_value: u64, difficulty: u64) -> bool {
    let target = compute_target(difficulty);
    hash_value < target
}

/// Compute the discovery bonus — a multiplier that rewards miners for
/// finding a particularly good (low) hash.
///
/// The bonus is the square root of `u64::MAX / (difficulty * hash_value)`,
/// clamped to a minimum of 1. This means a hash far below the target
/// yields a higher bonus, creating economic incentive to continue mining
/// even after the base reward is small.
pub fn compute_discovery_bonus(difficulty: u64, hash_value: u64) -> u64 {
    if difficulty == 0 || hash_value == 0 {
        return 1;
    }
    // Compute the ratio of the entire hash space to the product of difficulty
    // and the actual hash — a measure of how "far below target" this hash is.
    let ratio = u128::from(u64::MAX) / (u128::from(difficulty) * u128::from(hash_value));
    if ratio < 1 {
        return 1;
    }
    // Square-root scaling keeps the bonus sub-linear to prevent inflation spikes.
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