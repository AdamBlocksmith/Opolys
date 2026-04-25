//! # Adaptive difficulty retargeting for Opolys.
//!
//! Opolys has **no hard cap** on supply and no hardcoded difficulty schedules.
//! Difficulty emerges from two components:
//!
//! 1. **Retarget** — an adjustment every `EPOCH` blocks (1,024) that compares
//!    actual block times against the target interval.
//! 2. **Consensus floor** — `total_issued / bonded_stake`, a minimum difficulty
//!    that rises as more $OPL enters circulation relative to staked supply.
//!
//! The effective difficulty is the maximum of the retarget, the consensus floor,
//! and `MIN_DIFFICULTY` (which is the mathematical floor, not an arbitrary cap).
//! There is no maximum difficulty clamp — difficulty adjusts freely.
//!
//! The discovery bonus has been replaced by **vein yield**, computed in
//! `emission.rs`. This module no longer computes discovery bonuses.

use opolys_core::{MIN_DIFFICULTY, EPOCH, BlockHeight};

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
    /// floor, and the global `MIN_DIFFICULTY` (which is 1 — a mathematical
    /// floor, not an artificial clamp).
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

/// Retarget difficulty every `EPOCH` blocks by comparing actual block times
/// to the expected target interval.
///
/// If the epoch was too fast, difficulty increases; if too slow, it decreases.
/// There is **no clamp** on adjustments — difficulty adjusts freely based on
/// observed block times. The only floor is `MIN_DIFFICULTY` (1), which is a
/// mathematical requirement (difficulty 0 would make all hashes valid).
fn compute_retarget(current_difficulty: u64, current_height: BlockHeight, block_timestamps: &[u64]) -> u64 {
    // Not enough blocks for a retarget epoch yet — hold at current difficulty.
    if current_height < EPOCH {
        return current_difficulty.max(MIN_DIFFICULTY);
    }

    let epoch_start = current_height.saturating_sub(EPOCH);
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
    // Use milliseconds for precision: 84,375 ms * 1,024 blocks = 86,400,000 ms = 24 hours exactly
    let expected_time_ms = EPOCH * opolys_core::BLOCK_TARGET_TIME_MS;
    // Convert actual time to milliseconds for consistent comparison
    let actual_time_ms = actual_time.saturating_mul(1_000);

    // If timestamps are degenerate (zero elapsed time), spike difficulty.
    if actual_time_ms == 0 {
        return current_difficulty.saturating_mul(4);
    }

    // Standard retarget: scale difficulty proportionally to expected vs actual time.
    // If blocks were too fast (actual < expected), difficulty increases.
    // If blocks were too slow (actual > expected), difficulty decreases.
    // Uses u128 intermediate to prevent overflow on large difficulty values.
    let numerator = current_difficulty as u128 * expected_time_ms as u128;
    let denominator = actual_time_ms as u128;
    let new_difficulty = (numerator / denominator) as u64;

    // No maximum clamp — difficulty adjusts freely.
    // The only floor is MIN_DIFFICULTY (1), a mathematical requirement.
    new_difficulty.max(MIN_DIFFICULTY)
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
    fn check_pow_easy_difficulty() {
        assert!(check_proof_of_work(0, 1));
        assert!(!check_proof_of_work(u64::MAX, 1));
    }

    #[test]
    fn retarget_at_epoch_boundary() {
        // Timestamps spaced at target block time (84,375 ms ≈ 84.375 seconds)
        let timestamps: Vec<u64> = (0..=2000).map(|i| i * 84).collect();
        let new_diff = compute_retarget(100, 1024, &timestamps);
        assert!(new_diff >= MIN_DIFFICULTY);
    }

    #[test]
    fn retarget_no_clamp_max() {
        // If blocks are 10x too slow, difficulty should drop proportionally
        // without an artificial maximum clamp
        let timestamps: Vec<u64> = (0..=2000).map(|i| i * 840).collect();
        let new_diff = compute_retarget(100, 1024, &timestamps);
        // 840s per block × 1,024 blocks = 860,160s actual
        // Expected: 84,375ms × 1,024 = 86,400,000ms ≈ 86,400s
        // Ratio: 86,400,000 / 860,160,000 ≈ 0.1x, so difficulty drops from 100 to ~10
        assert!(new_diff < 100, "Difficulty should drop when blocks are too slow: got {}", new_diff);
    }

    #[test]
    fn compute_next_difficulty_integrates_consensus_floor() {
        let timestamps: Vec<u64> = (0..=2000).map(|i| i * 84).collect();
        let result = compute_next_difficulty(100, 1024, &timestamps, 10_000_000, 1_000_000);
        assert!(result.effective_difficulty() >= 10);
    }

    #[test]
    fn min_difficulty_is_mathematical_floor() {
        // MIN_DIFFICULTY = 1 is the only floor — difficulty can freely adjust above it
        assert_eq!(MIN_DIFFICULTY, 1);
    }

    #[test]
    fn epoch_is_1024() {
        assert_eq!(EPOCH, 1024);
    }
}