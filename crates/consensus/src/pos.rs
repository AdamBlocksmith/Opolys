//! # Proof-of-stake validator management for Opolys.
//!
//! Opolys validators bond stake as collateral and earn block rewards
//! proportional to their **weight** = `stake × (1 + ln(1 + age_years))`.
//! This gives a logarithmic — not linear — seniority bonus: early validators
//! earn more, but the marginal gain diminishes over time, preventing permanent
//! dominance by first-movers.
//!
//! **Slashing is narrowly scoped to double-signing only.** No governance
//! body can slash for other reasons. A slashed validator's entire stake is
//! burned (not confiscated to any treasury), permanently removing it from
//! circulation.
//!
//! Block producers are selected via weighted random sampling, where the seed
//! is derived from on-chain entropy. There are no rounds, no schedules, and
//! no fixed validator sets — just continuous weighted selection.

use opolys_core::{FlakeAmount, ObjectId, ValidatorStatus, MIN_BOND_STAKE};
use borsh::{BorshSerialize, BorshDeserialize};
use std::collections::HashMap;

/// Information about a bonded validator.
///
/// Validators lock stake as collateral and earn block rewards proportional
/// to their weight = stake × (1 + ln(1 + age_years)). Only double-signing
/// triggers slashing (full stake burned).
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct ValidatorInfo {
    /// The validator's on-chain identity (Blake3 hash of public key).
    pub object_id: ObjectId,
    /// Amount of $OPL (in flakes) locked as collateral. Slashed to zero on
    /// double-signing — the entire amount is burned, not transferred.
    pub stake: FlakeAmount,
    /// Block height at which the validator first bonded.
    pub bonded_at_height: u64,
    /// Unix timestamp at which the validator first bonded, used to compute
    /// seniority for weight calculations.
    pub bonded_at_timestamp: u64,
    /// Current lifecycle status: Bonding → Active → (Slashed | Unbonded).
    pub status: ValidatorStatus,
    /// Height of the most recent block this validator signed, updated on
    /// each signature to track liveness.
    pub last_signed_height: u64,
}

impl ValidatorInfo {
    /// Compute the validator's seniority in years based on the time elapsed
    /// since bonding. Returns 0.0 if `current_timestamp` is at or before the
    /// bonding timestamp.
    pub fn age_years(&self, current_timestamp: u64) -> f64 {
        if current_timestamp <= self.bonded_at_timestamp {
            return 0.0;
        }
        let age_secs = current_timestamp - self.bonded_at_timestamp;
        // Use 365.25 days/year to account for leap years accurately.
        age_secs as f64 / (365.25 * 24.0 * 3600.0)
    }
}

/// The set of all bonded validators, supporting bonding, unbonding,
/// activation, slashing, and weighted block-producer selection.
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

    /// Bond a new validator with the given stake. Fails if the stake is below
    /// `MIN_BOND_STAKE` or if the validator is already bonded. New validators
    /// start in `Bonding` status and must be activated separately.
    pub fn bond(
        &mut self,
        object_id: ObjectId,
        stake: FlakeAmount,
        height: u64,
        timestamp: u64,
    ) -> Result<(), String> {
        if stake < MIN_BOND_STAKE {
            return Err(format!(
                "Insufficient stake: need {}, have {}",
                MIN_BOND_STAKE, stake
            ));
        }

        if self.validators.contains_key(&object_id) {
            return Err("Validator already bonded".to_string());
        }

        self.validators.insert(object_id.clone(), ValidatorInfo {
            object_id,
            stake,
            bonded_at_height: height,
            bonded_at_timestamp: timestamp,
            status: ValidatorStatus::Bonding,
            last_signed_height: 0,
        });

        Ok(())
    }

    /// Unbond a validator, removing them from the set and returning their info.
    /// Fails if the validator is not found.
    pub fn unbond(&mut self, object_id: &ObjectId) -> Result<ValidatorInfo, String> {
        let info = self.validators.remove(object_id)
            .ok_or_else(|| "Validator not bonded".to_string())?;
        Ok(info)
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

    /// Slash a validator for double-signing. The validator's entire stake is
    /// **burned** (set to zero), not transferred to any other party. Their
    /// status is set to `Slashed` and they are no longer eligible for block
    /// production.
    ///
    /// This is the **only** slashing condition in Opolys — there is no
    /// governance, no liveness slashing, and no other penalties.
    pub fn slash(&mut self, object_id: &ObjectId) -> Result<FlakeAmount, String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not found".to_string())?;
        let slashed_amount = validator.stake;
        validator.status = ValidatorStatus::Slashed;
        // Slash burns the entire stake — it leaves circulating supply entirely.
        validator.stake = 0;
        Ok(slashed_amount)
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
            .map(|v| v.stake)
            .sum()
    }

    /// Return all validators currently in `Active` status.
    pub fn active_validators(&self) -> Vec<&ValidatorInfo> {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .collect()
    }

    /// Compute the total weight of all active validators. Weight is
    /// `stake × (1 + ln(1 + age_years))`, giving a logarithmic seniority
    /// bonus that diminishes over time rather than compounding endlessly.
    pub fn total_weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .map(|v| crate::emission::compute_validator_weight(v.stake, v.age_years(current_timestamp)))
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
    /// Each active validator's weight determines their probability of being
    /// selected. The `seed` parameter provides on-chain entropy to make the
    /// selection deterministic and verifiable. Returns `None` if there are
    /// no active validators.
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
            .map(|v| crate::emission::compute_validator_weight(v.stake, v.age_years(current_timestamp)))
            .sum();

        if total_weight == 0 {
            return None;
        }

        // Cumulative weighted selection: walk through validators, accumulating
        // weight until the cumulative total exceeds the random target.
        let mut cumulative = 0u64;
        let target = seed % total_weight;
        for v in &active {
            let weight = crate::emission::compute_validator_weight(v.stake, v.age_years(current_timestamp));
            cumulative += weight;
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
    fn bond_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        assert_eq!(vs.validator_count(), 1);
        assert_eq!(vs.get_validator(&id).unwrap().stake, MIN_BOND_STAKE);
    }

    #[test]
    fn bond_insufficient_stake() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        assert!(vs.bond(id, 100, 0, 0).is_err());
    }

    #[test]
    fn unbond_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        let info = vs.unbond(&id).unwrap();
        assert_eq!(info.stake, MIN_BOND_STAKE);
        assert_eq!(vs.validator_count(), 0);
    }

    #[test]
    fn slash_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        let slashed = vs.slash(&id).unwrap();
        assert_eq!(slashed, MIN_BOND_STAKE);
        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.stake, 0);
        assert_eq!(v.status, ValidatorStatus::Slashed);
    }

    #[test]
    fn total_bonded_stake() {
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
    fn stake_coverage() {
        let coverage = crate::emission::compute_stake_coverage(500_000, 1_000_000);
        assert!((coverage - 0.5).abs() < 0.001);
    }
}