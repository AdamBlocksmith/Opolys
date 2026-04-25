//! # Proof-of-stake validator management for Opolys.
//!
//! Opolys validators bond stake as collateral and earn block rewards
//! proportional to their **weight**, which is the sum of per-entry weights:
//!
//! `weight = Σ entry.stake × (1 + ln(1 + entry.age_years))`
//!
//! Each bond entry has its own seniority clock that starts from zero when the
//! entry is created. This means:
//! - A fresh top-up earns no seniority bonus initially
//! - Older entries earn proportionally more per-coin
//! - The marginal gain diminishes logarithmically, preventing permanent dominance
//!
//! **Unbonding is FIFO** — when a validator unbonds, the oldest entries are
//! consumed first. If the unbond amount exceeds an entry's stake, that entry
//! is fully consumed and the remainder comes from the next oldest. Split
//! entries keep their original `bonded_at_timestamp` for weight calculation.
//! The 1 OPL minimum only applies to new bond entries, not to residuals from
//! FIFO splits.
//!
//! **Slashing is narrowly scoped to double-signing only.** No governance
//! body can slash for other reasons. A slashed validator's entire stake across
//! all entries is burned (not confiscated to any treasury), permanently
//! removing it from circulation.
//!
//! Block producers are selected via weighted random sampling, where the seed
//! is derived from on-chain entropy. There are no rounds, no schedules, and
//! no fixed validator sets — just continuous weighted selection.

use opolys_core::{FlakeAmount, ObjectId, ValidatorStatus, MIN_BOND_STAKE};
use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// A single bond entry within a validator's stake.
///
/// Each entry has its own stake amount, bonding timestamp, and seniority clock.
/// Entries are consumed in FIFO order during unbonding — oldest first.
/// Split entries retain their original `bonded_at_timestamp` so they
/// continue earning seniority weight as if they had never been split.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BondEntry {
    /// Amount of OPL (in flakes) locked in this entry.
    pub stake: FlakeAmount,
    /// Block height at which this entry was bonded.
    pub bonded_at_height: u64,
    /// Unix timestamp at which this entry was bonded, used to compute
    /// seniority for weight calculations.
    pub bonded_at_timestamp: u64,
}

impl BondEntry {
    /// Compute this entry's seniority in milli-years (× 1000) based on the
    /// time elapsed since bonding. Returns 0 if `current_timestamp` is at or
    /// before the bonding timestamp.
    ///
    /// Returns milli-years (not fractional years) to maintain integer-only
    /// arithmetic throughout consensus code.
    pub fn age_years_milli(&self, current_timestamp: u64) -> u64 {
        if current_timestamp <= self.bonded_at_timestamp {
            return 0;
        }
        let age_secs = current_timestamp - self.bonded_at_timestamp;
        // Convert seconds to milli-years: age_secs × 1000 / (365.25 × 86400)
        // Using integer arithmetic: age_secs × 1000 / 31_557_600
        (age_secs as u128 * 1000 / 31_557_600) as u64
    }

    /// Compute this entry's weight: `stake × (1 + ln(1 + age_years))`.
    ///
    /// Uses integer-only arithmetic via `ln_milli` in the emission module.
    /// Older entries earn a logarithmic seniority bonus that diminishes over
    /// time, preventing permanent dominance by early stakers.
    pub fn weight(&self, current_timestamp: u64) -> FlakeAmount {
        crate::emission::compute_validator_weight(self.stake, self.age_years_milli(current_timestamp))
    }
}

/// Information about a bonded validator.
///
/// Validators hold one or more bond entries, each with its own stake amount
/// and seniority clock. The total weight is the sum of per-entry weights,
/// giving a logarithmic seniority bonus that diminishes over time.
///
/// Only double-signing triggers slashing (all entries burned). Slashed stake
/// is removed from circulation, not transferred to any treasury.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct ValidatorInfo {
    /// The validator's on-chain identity (Blake3 hash of public key).
    pub object_id: ObjectId,
    /// The validator's bond entries, each with its own stake and seniority.
    /// Entries are sorted by `bonded_at_timestamp` (FIFO order).
    pub entries: Vec<BondEntry>,
    /// Current lifecycle status: Bonding → Active → (Slashed | Unbonded).
    pub status: ValidatorStatus,
    /// Height of the most recent block this validator signed, updated on
    /// each signature to track liveness.
    pub last_signed_height: u64,
}

impl ValidatorInfo {
    /// Create a new validator with their first bond entry.
    fn new(object_id: ObjectId, stake: FlakeAmount, height: u64, timestamp: u64) -> Self {
        ValidatorInfo {
            object_id,
            entries: vec![BondEntry {
                stake,
                bonded_at_height: height,
                bonded_at_timestamp: timestamp,
            }],
            status: ValidatorStatus::Bonding,
            last_signed_height: 0,
        }
    }

    /// Total stake across all bond entries.
    pub fn total_stake(&self) -> FlakeAmount {
        self.entries.iter().map(|e| e.stake).sum()
    }

    /// Compute the validator's total weight as the sum of per-entry weights.
    ///
    /// Each entry's weight is `stake × (1 + ln(1 + age_years))`, giving
    /// a logarithmic seniority bonus that diminishes over time.
    pub fn weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.entries.iter().map(|e| e.weight(current_timestamp)).sum()
    }

    /// Add a new bond entry (top-up) to this validator.
    ///
    /// Each new entry must meet the `MIN_BOND_STAKE` minimum (1 OPL).
    /// The entry gets its own seniority clock starting from zero.
    /// If an entry with the same `bonded_at_timestamp` already exists,
    /// stakes are merged (auto-merge) to reduce entry count.
    fn add_entry(&mut self, stake: FlakeAmount, height: u64, timestamp: u64) {
        // Auto-merge: if an entry with the same timestamp exists, combine stakes
        if let Some(existing) = self.entries.iter_mut().find(|e| e.bonded_at_timestamp == timestamp) {
            existing.stake = existing.stake.saturating_add(stake);
            return;
        }
        // Otherwise, insert in sorted order by timestamp (FIFO)
        let entry = BondEntry {
            stake,
            bonded_at_height: height,
            bonded_at_timestamp: timestamp,
        };
        let pos = self.entries.iter().position(|e| e.bonded_at_timestamp > timestamp).unwrap_or(self.entries.len());
        self.entries.insert(pos, entry);
    }

    /// Unbond `amount` Flakes from this validator using FIFO order.
    ///
    /// Consumes the oldest entries first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the
    /// next oldest. Split entries keep their original `bonded_at_timestamp`.
    /// Returns the actual amount unbonded (may be less than requested if
    /// total stake is insufficient).
    fn unbond_fifo(&mut self, amount: FlakeAmount) -> FlakeAmount {
        let mut remaining = amount;
        let mut consumed = 0u64;

        while remaining > 0 && !self.entries.is_empty() {
            let entry_stake = self.entries[consumed as usize].stake;
            if entry_stake <= remaining {
                // Full consumption of this entry
                remaining -= entry_stake;
                consumed += 1;
            } else {
                // Partial consumption — split the entry
                self.entries[consumed as usize].stake -= remaining;
                remaining = 0;
            }
        }

        // Remove consumed entries from the front
        self.entries.drain(0..consumed as usize);

        amount.saturating_sub(remaining)
    }

    /// Find a specific bond entry by index.
    pub fn get_entry(&self, index: usize) -> Option<&BondEntry> {
        self.entries.get(index)
    }
}

/// The set of all bonded validators, supporting bonding, unbonding,
/// activating, slashing, and weighted block-producer selection.
///
/// Only **double-signing** triggers slashing in Opolys — no other offense
/// results in stake removal. Slashed stake is burned (removed from supply),
/// not sent to any entity.
#[derive(Debug)]
pub struct ValidatorSet {
    validators: HashMap<ObjectId, ValidatorInfo>,
}

impl ValidatorSet {
    /// Create an empty validator set.
    pub fn new() -> Self {
        ValidatorSet {
            validators: HashMap::new(),
        }
    }

    /// Bond stake as a validator entry. If the validator doesn't exist, creates
    /// a new validator with this as their first entry (status: Bonding). If the
    /// validator already exists, adds to the existing entry (auto-merge) if same
    /// timestamp, or creates a new entry (top-up) with its own seniority clock.
    ///
    /// Each **new** entry must meet `MIN_BOND_STAKE` (1 OPL). Merged entries
    /// have no minimum since they may be residuals from FIFO splits.
    pub fn bond(
        &mut self,
        object_id: ObjectId,
        stake: FlakeAmount,
        height: u64,
        timestamp: u64,
    ) -> Result<(), String> {
        if stake < MIN_BOND_STAKE {
            return Err(format!(
                "Insufficient stake per entry: need {}, have {}",
                MIN_BOND_STAKE, stake
            ));
        }

        if let Some(validator) = self.validators.get_mut(&object_id) {
            // Top-up or auto-merge
            validator.add_entry(stake, height, timestamp);
        } else {
            // New validator: create with first bond entry
            self.validators.insert(object_id.clone(), ValidatorInfo::new(object_id, stake, height, timestamp));
        }

        Ok(())
    }

    /// Unbond `amount` Flakes from a validator using FIFO order.
    ///
    /// The oldest entries are consumed first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the
    /// next oldest. The actual unbonded amount is returned (may be less than
    /// requested if total stake is insufficient).
    ///
    /// After a `UNBONDING_DELAY_BLOCKS` delay (1 epoch = 1,024 blocks),
    /// the unbonded stake is returned to the sender. Unbonding stake still
    /// earns rewards during the delay period.
    ///
    /// If the validator has no remaining entries after unbonding, they are
    /// removed from the set entirely.
    pub fn unbond_amount(
        &mut self,
        object_id: &ObjectId,
        amount: FlakeAmount,
    ) -> Result<FlakeAmount, String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not bonded".to_string())?;

        let total_stake = validator.total_stake();
        if total_stake == 0 {
            return Err("Validator has no stake".to_string());
        }

        // Unbond up to the amount available
        let actual_amount = amount.min(total_stake);
        let unbonded = validator.unbond_fifo(actual_amount);

        // If no entries remain, remove the validator entirely
        if validator.entries.is_empty() {
            self.validators.remove(object_id);
        }

        Ok(unbonded)
    }

    /// Activate a validator that is currently in `Bonding` status. This
    /// transitions them to `Active`, making them eligible for block producer
    /// selection and reward distribution.
    pub fn activate(&mut self, object_id: &ObjectId, height: u64) -> Result<(), String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not found".to_string())?;
        if validator.status != ValidatorStatus::Bonding {
            return Err("Validator not in bonding state".to_string());
        }
        validator.status = ValidatorStatus::Active;
        validator.last_signed_height = height;
        Ok(())
    }

    /// Slash a validator for double-signing. All entries' stakes are
    /// **burned** (set to zero), not transferred to any other party. Their
    /// status is set to `Slashed` and they are no longer eligible for block
    /// production.
    ///
    /// This is the **only** slashing condition in Opolys — there is no
    /// governance, no liveness slashing, and no other penalties.
    pub fn slash(&mut self, object_id: &ObjectId) -> Result<FlakeAmount, String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not found".to_string())?;
        let total_slashed = validator.total_stake();
        validator.status = ValidatorStatus::Slashed;
        for entry in &mut validator.entries {
            entry.stake = 0;
        }
        Ok(total_slashed)
    }

    /// Look up a validator by their ObjectId.
    pub fn get_validator(&self, object_id: &ObjectId) -> Option<&ValidatorInfo> {
        self.validators.get(object_id)
    }

    /// Total stake across all Bonding and Active validators. Used to
    /// compute stake coverage, which determines the PoW/PoS reward split.
    pub fn total_bonded_stake(&self) -> FlakeAmount {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active || v.status == ValidatorStatus::Bonding)
            .map(|v| v.total_stake())
            .sum()
    }

    /// Return all validators currently in `Active` status.
    pub fn active_validators(&self) -> Vec<&ValidatorInfo> {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .collect()
    }

    /// Compute the total weight of all active validators. Weight is the sum
    /// of per-entry weights: `Σ entry.stake × (1 + ln(1 + entry.age_years))`,
    /// giving a logarithmic seniority bonus per entry that diminishes over time
    /// rather than compounding endlessly.
    pub fn total_weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .map(|v| v.weight(current_timestamp))
            .sum()
    }

    /// Number of validators in the set (regardless of status).
    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// Return all validators as a serializable Vec. Used for persistence.
    pub fn all_validators(&self) -> Vec<ValidatorInfo> {
        self.validators.values().cloned().collect()
    }

    /// Load validators from a serialized Vec. Used for state restoration.
    pub fn load_from_validators(validators: Vec<ValidatorInfo>) -> Self {
        let mut set = ValidatorSet::new();
        for v in validators {
            set.validators.insert(v.object_id.clone(), v);
        }
        set
    }

    /// Select the next block producer via weighted random sampling.
    ///
    /// Each active validator's total weight (sum of per-entry weights)
    /// determines their probability of being selected. The `seed` parameter
    /// provides on-chain entropy to make the selection deterministic and
    /// verifiable. Returns `None` if there are no active validators.
    pub fn select_block_producer(
        &self,
        current_timestamp: u64,
        seed: u64,
    ) -> Option<&ValidatorInfo> {
        let active: Vec<&ValidatorInfo> = self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .collect();

        if active.is_empty() {
            return None;
        }

        let total_weight: FlakeAmount = active.iter()
            .map(|v| v.weight(current_timestamp))
            .sum();

        if total_weight == 0 {
            return None;
        }

        // Cumulative weighted selection: walk through validators, accumulating
        // weight until the cumulative total exceeds the random target.
        let mut cumulative = 0u64;
        let target = seed % total_weight;
        for v in &active {
            cumulative += v.weight(current_timestamp);
            if cumulative > target {
                return Some(*v);
            }
        }

        // Fallback: if no validator was selected due to rounding, pick the last active one.
        active.last().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_crypto::hash_to_object_id;

    fn test_id(seed: &[u8]) -> ObjectId {
        hash_to_object_id(seed)
    }

    #[test]
    fn bond_new_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        assert_eq!(vs.validator_count(), 1);
        assert_eq!(vs.get_validator(&id).unwrap().total_stake(), MIN_BOND_STAKE);
        assert_eq!(vs.get_validator(&id).unwrap().entries.len(), 1);
    }

    #[test]
    fn bond_insufficient_stake_per_entry() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        // MIN_BOND_STAKE is now 1 OPL = 1,000,000 flakes
        assert!(vs.bond(id, 100, 0, 0).is_err());
    }

    #[test]
    fn top_up_bond_adds_entry() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up: add a second entry at a different timestamp
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap();

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.entries.len(), 2);
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);

        // First entry (timestamp 0) and second entry (timestamp 1000)
        assert_eq!(v.entries[0].bonded_at_timestamp, 0);
        assert_eq!(v.entries[1].bonded_at_timestamp, 1000);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE);
        assert_eq!(v.entries[1].stake, MIN_BOND_STAKE * 2);
    }

    #[test]
    fn auto_merge_same_timestamp() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 100).unwrap();

        // Top-up at same timestamp — should auto-merge
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 5, 100).unwrap();

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.entries.len(), 1); // Merged, not two entries
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);
    }

    #[test]
    fn unbond_fifo_consumes_oldest_first() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();       // Entry 1: 1 OPL at t=0
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap(); // Entry 2: 2 OPL at t=1000

        // Unbond 1.5 OPL — should fully consume entry 1 (1 OPL) and 0.5 OPL from entry 2
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE + MIN_BOND_STAKE / 2).unwrap();

        // Should return all requested amount since total stake > amount
        assert_eq!(unbonded, MIN_BOND_STAKE + MIN_BOND_STAKE / 2);

        let v = vs.get_validator(&id).unwrap();
        // Entry 1 is gone, entry 2 has 1.5 OPL remaining with original timestamp
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE * 2 - MIN_BOND_STAKE / 2);
        // Split entry keeps original timestamp for seniority
        assert_eq!(v.entries[0].bonded_at_timestamp, 1000);
    }

    #[test]
    fn unbond_fifo_removes_validator_when_empty() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Unbond the entire stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);
        assert_eq!(vs.validator_count(), 0);
    }

    #[test]
    fn unbond_more_than_stake() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Try to unbond more than total stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE * 10).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE); // Only unbond what's available
        assert_eq!(vs.validator_count(), 0);
    }

    #[test]
    fn unbond_nonexistent_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        let result = vs.unbond_amount(&id, MIN_BOND_STAKE);
        assert!(result.is_err());
    }

    #[test]
    fn slash_validator_burns_all_entries() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap();

        let slashed = vs.slash(&id).unwrap();
        assert_eq!(slashed, MIN_BOND_STAKE * 3);

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.total_stake(), 0);
        assert_eq!(v.status, ValidatorStatus::Slashed);
        for entry in &v.entries {
            assert_eq!(entry.stake, 0);
        }
    }

    #[test]
    fn total_bonded_stake_with_multiple_entries() {
        let mut vs = ValidatorSet::new();
        let id1 = test_id(b"validator1");
        let id2 = test_id(b"validator2");
        vs.bond(id1, MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id2, MIN_BOND_STAKE * 2, 0, 0).unwrap();
        assert_eq!(vs.total_bonded_stake(), MIN_BOND_STAKE * 3);
    }

    #[test]
    fn select_block_producer() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();
        let producer = vs.select_block_producer(0, 42);
        assert!(producer.is_some());
        assert_eq!(producer.unwrap().object_id, id);
    }

    #[test]
    fn per_entry_seniority_increases_weight() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 100).unwrap();
        vs.activate(&id, 1).unwrap();

        // At timestamp 100, age = 0, weight = stake × 1.0
        let weight_at_bond = vs.total_weight(100);
        // At timestamp 100 + 1 year, age = 1.0, weight = stake × (1 + ln(2))
        let one_year_secs = (365.25 * 24.0 * 3600.0) as u64;
        let weight_after_year = vs.total_weight(100 + one_year_secs);

        assert!(weight_after_year > weight_at_bond);
    }

    #[test]
    fn top_up_entry_has_zero_seniority() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up at timestamp 1,000,000 (~11.5 days)
        let top_up_time: u64 = 1_000_000;
        vs.bond(id.clone(), MIN_BOND_STAKE, 100, top_up_time).unwrap();

        let v = vs.get_validator(&id).unwrap();
        // Check age at ~1 year (31,557,600 seconds) — enough for measurable milli-years
        let check_time: u64 = 31_557_600;
        let age_0_milli = v.entries[0].age_years_milli(check_time);
        let age_1_milli = v.entries[1].age_years_milli(check_time);
        // Entry bonded at genesis should have ~31.7 milli-years at check_time
        assert!(age_0_milli > 0, "Entry bonded at genesis should have age after 1 year");
        // Top-up entry should have ~30.5 milli-years (1 year - 11.5 days)
        assert!(age_1_milli > 0, "Top-up entry should have age after 1 year");
        // At the exact top-up time, the new entry has zero seniority
        assert_eq!(v.entries[1].age_years_milli(top_up_time), 0);
    }

    #[test]
    fn stake_coverage() {
        let coverage = crate::emission::compute_stake_coverage(500_000, 1_000_000);
        assert!((coverage - 0.5).abs() < 0.001);
    }

    #[test]
    fn unbond_fifo_partial_from_second_entry() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        // Entry 1: 1 OPL, Entry 2: 3 OPL
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 3, 100, 2000).unwrap();

        // Unbond 2 OPL: consumes entry 1 (1 OPL) + 1 OPL from entry 2
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE * 2).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE * 2);

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE * 2); // 3 - 1 = 2
        assert_eq!(v.entries[0].bonded_at_timestamp, 2000); // Keeps original timestamp
    }
}