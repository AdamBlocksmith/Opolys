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
//! Validators can hold multiple bond entries and unbond them individually by
//! `bond_id`. Pools are a market innovation — the protocol provides per-entry
//! bonds, community builds pooling off-chain.
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
/// Validators can hold multiple entries (top-ups), and unbond them individually
/// by `bond_id`. The `bond_id` is a per-validator auto-incrementing counter
/// that uniquely identifies each entry.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BondEntry {
    /// Unique identifier for this entry, auto-incremented per validator.
    pub bond_id: u64,
    /// Amount of OPL (in flakes) locked in this entry.
    pub stake: FlakeAmount,
    /// Block height at which this entry was bonded.
    pub bonded_at_height: u64,
    /// Unix timestamp at which this entry was bonded, used to compute
    /// seniority for weight calculations.
    pub bonded_at_timestamp: u64,
}

impl BondEntry {
    /// Compute this entry's seniority in years based on the time elapsed
    /// since bonding. Returns 0.0 if `current_timestamp` is at or before the
    /// bonding timestamp.
    pub fn age_years(&self, current_timestamp: u64) -> f64 {
        if current_timestamp <= self.bonded_at_timestamp {
            return 0.0;
        }
        let age_secs = current_timestamp - self.bonded_at_timestamp;
        age_secs as f64 / (365.25 * 24.0 * 3600.0)
    }

    /// Compute this entry's weight: `stake × (1 + ln(1 + age_years))`.
    ///
    /// Older entries earn a logarithmic seniority bonus that diminishes over
    /// time, preventing permanent dominance by early stakers.
    pub fn weight(&self, current_timestamp: u64) -> FlakeAmount {
        crate::emission::compute_validator_weight(self.stake, self.age_years(current_timestamp))
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
    pub entries: Vec<BondEntry>,
    /// Current lifecycle status: Bonding → Active → (Slashed | Unbonded).
    pub status: ValidatorStatus,
    /// Height of the most recent block this validator signed, updated on
    /// each signature to track liveness.
    pub last_signed_height: u64,
    /// Auto-incrementing counter for the next bond entry's ID.
    next_bond_id: u64,
}

impl ValidatorInfo {
    /// Create a new validator with their first bond entry.
    fn new(object_id: ObjectId, stake: FlakeAmount, height: u64, timestamp: u64) -> Self {
        ValidatorInfo {
            object_id,
            entries: vec![BondEntry {
                bond_id: 0,
                stake,
                bonded_at_height: height,
                bonded_at_timestamp: timestamp,
            }],
            status: ValidatorStatus::Bonding,
            last_signed_height: 0,
            next_bond_id: 1,
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
    /// Each entry must meet the `MIN_BOND_STAKE` minimum. The entry gets
    /// its own `bond_id` and starts with zero seniority.
    fn add_entry(&mut self, stake: FlakeAmount, height: u64, timestamp: u64) {
        let bond_id = self.next_bond_id;
        self.next_bond_id += 1;
        self.entries.push(BondEntry {
            bond_id,
            stake,
            bonded_at_height: height,
            bonded_at_timestamp: timestamp,
        });
    }

    /// Remove a specific bond entry by its `bond_id`, returning the stake
    /// amount that was in that entry. Returns `None` if the bond_id doesn't exist.
    fn remove_entry(&mut self, bond_id: u64) -> Option<FlakeAmount> {
        let idx = self.entries.iter().position(|e| e.bond_id == bond_id)?;
        Some(self.entries.remove(idx).stake)
    }

    /// Find a specific bond entry by its ID.
    pub fn get_entry(&self, bond_id: u64) -> Option<&BondEntry> {
        self.entries.iter().find(|e| e.bond_id == bond_id)
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
    /// validator already exists, adds a new bond entry (top-up) with its own
    /// seniority clock starting from zero.
    ///
    /// Each entry must meet `MIN_BOND_STAKE` (100 OPL). Returns an error if
    /// the stake is below the minimum.
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
            // Top-up: add a new bond entry to existing validator
            validator.add_entry(stake, height, timestamp);
        } else {
            // New validator: create with first bond entry
            self.validators.insert(object_id.clone(), ValidatorInfo::new(object_id, stake, height, timestamp));
        }

        Ok(())
    }

    /// Unbond a specific bond entry by `bond_id`, returning that entry's stake.
    /// If the validator has no remaining entries after removal, the validator
    /// is removed from the set entirely.
    ///
    /// Returns an error if the validator or bond_id is not found.
    pub fn unbond_entry(
        &mut self,
        object_id: &ObjectId,
        bond_id: u64,
    ) -> Result<FlakeAmount, String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not bonded".to_string())?;

        let stake = validator.remove_entry(bond_id)
            .ok_or_else(|| format!("Bond entry {} not found", bond_id))?;

        // If no entries remain, remove the validator entirely
        if validator.entries.is_empty() {
            self.validators.remove(object_id);
        }

        Ok(stake)
    }

    /// Unbond a validator entirely, removing all entries and returning the
    /// total stake across all bond entries.
    ///
    /// Fails if the validator is not found.
    pub fn unbond(&mut self, object_id: &ObjectId) -> Result<ValidatorInfo, String> {
        self.validators.remove(object_id)
            .ok_or_else(|| "Validator not bonded".to_string())
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
        assert!(vs.bond(id, 100, 0, 0).is_err());
    }

    #[test]
    fn top_up_bond_adds_entry() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up: add a second entry
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap();

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.entries.len(), 2);
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);

        // First entry has bond_id 0, second has bond_id 1
        assert_eq!(v.entries[0].bond_id, 0);
        assert_eq!(v.entries[1].bond_id, 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE);
        assert_eq!(v.entries[1].stake, MIN_BOND_STAKE * 2);
    }

    #[test]
    fn top_up_minimum_stake_enforcement() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Top-up below minimum should fail
        assert!(vs.bond(id, 50, 100, 1000).is_err());
    }

    #[test]
    fn unbond_specific_entry() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap();

        // Unbond entry #0
        let returned = vs.unbond_entry(&id, 0).unwrap();
        assert_eq!(returned, MIN_BOND_STAKE);

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 2);
        assert_eq!(v.entries[0].bond_id, 1); // only entry #1 remains
    }

    #[test]
    fn unbond_last_entry_removes_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Unbond the only entry
        let returned = vs.unbond_entry(&id, 0).unwrap();
        assert_eq!(returned, MIN_BOND_STAKE);
        assert_eq!(vs.validator_count(), 0);
    }

    #[test]
    fn unbond_nonexistent_bond_id() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        let result = vs.unbond_entry(&id, 999);
        assert!(result.is_err());
    }

    #[test]
    fn unbond_nonexistent_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        let result = vs.unbond_entry(&id, 0);
        assert!(result.is_err());
    }

    #[test]
    fn unbond_all_entries() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        let info = vs.unbond(&id).unwrap();
        assert_eq!(info.total_stake(), MIN_BOND_STAKE);
        assert_eq!(vs.validator_count(), 0);
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
        // All entries have zero stake
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

        // Top-up at timestamp 1000
        vs.bond(id.clone(), MIN_BOND_STAKE, 100, 1000).unwrap();

        // At timestamp 1000, first entry has 1000s seniority, second has 0
        let v = vs.get_validator(&id).unwrap();
        let age_0 = v.entries[0].age_years(1000);
        let age_1 = v.entries[1].age_years(1000);
        assert!(age_0 > 0.0);
        assert_eq!(age_1, 0.0);
    }

    #[test]
    fn stake_coverage() {
        let coverage = crate::emission::compute_stake_coverage(500_000, 1_000_000);
        assert!((coverage - 0.5).abs() < 0.001);
    }

    #[test]
    fn get_entry_by_bond_id() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap();

        let v = vs.get_validator(&id).unwrap();
        let entry0 = v.get_entry(0).unwrap();
        assert_eq!(entry0.stake, MIN_BOND_STAKE);
        assert_eq!(entry0.bond_id, 0);

        let entry1 = v.get_entry(1).unwrap();
        assert_eq!(entry1.stake, MIN_BOND_STAKE * 2);
        assert_eq!(entry1.bond_id, 1);

        assert!(v.get_entry(999).is_none());
    }
}