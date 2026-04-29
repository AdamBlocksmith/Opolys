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

use opolys_core::{FlakeAmount, ObjectId, ValidatorStatus, MIN_BOND_STAKE, EPOCH, MAX_ACTIVE_VALIDATORS};
use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use opolys_crypto::Blake3Hasher;

/// A pending unbonding entry that matures after a delay of EPOCH blocks.
///
/// When a validator unbonds, the stake is not returned immediately. Instead,
/// it enters the unbonding queue and matures after `UNBONDING_DELAY_BLOCKS`
/// (1,024 blocks = one epoch). Once matured, the stake is returned to the
/// account that originally bonded it.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct PendingUnbond {
    /// The account that will receive the unbonded stake.
    pub account: ObjectId,
    /// Amount of Flakes to return when this entry matures.
    pub amount: FlakeAmount,
    /// Block height at which this entry matures and can be claimed.
    pub matures_at: BlockHeight,
}

/// Block height type alias — re-exported from core for convenience.
pub type BlockHeight = u64;

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
/// Double-signing triggers graduated slashing: 10% burn on first offense,
/// 33% burn + suspension on second, 100% burn + permanent Slashed on third+.
/// Slashed stake is removed from circulation, not transferred to any treasury.
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
    /// Number of double-sign offenses committed. Resets to zero if the
    /// validator goes 10,240 blocks clean after the last slash.
    pub slash_offense_count: u32,
    /// Block height at which the most recent slash was applied.
    /// Used to determine whether the offense counter should reset.
    pub last_slash_height: u64,
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
            slash_offense_count: 0,
            last_slash_height: 0,
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
    /// Pending unbonding entries that mature after EPOCH blocks.
    pub unbonding_queue: Vec<PendingUnbond>,
}

impl ValidatorSet {
    /// Create an empty validator set.
    pub fn new() -> Self {
        ValidatorSet {
            validators: HashMap::new(),
            unbonding_queue: Vec::new(),
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
            if validator.status == ValidatorStatus::Slashed {
                return Err("Slashed validators cannot re-bond".to_string());
            }
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
    /// next oldest. The unbonded stake enters the unbonding queue and will
    /// mature after `UNBONDING_DELAY_BLOCKS` (1,024 blocks = one epoch).
    ///
    /// If the validator has no remaining entries after unbonding, they are
    /// removed from the validator set but not slashed.
    pub fn unbond_amount(
        &mut self,
        object_id: &ObjectId,
        amount: FlakeAmount,
        current_height: u64,
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

        // Enqueue the unbonding entry with a maturation height
        let matures_at = current_height.saturating_add(EPOCH);
        self.unbonding_queue.push(PendingUnbond {
            account: object_id.clone(),
            amount: unbonded,
            matures_at,
        });

        // If no entries remain, remove the validator entirely
        if validator.entries.is_empty() {
            self.validators.remove(object_id);
        }

        Ok(unbonded)
    }

    /// Process all matured unbonding entries at the given block height.
    ///
    /// Returns a Vec of (account, amount) pairs for entries that have matured.
    /// The caller is responsible for crediting the accounts.
    pub fn process_matured_unbonds(&mut self, current_height: u64) -> Vec<(ObjectId, FlakeAmount)> {
        let mut matured = Vec::new();
        let mut remaining = Vec::new();

        for entry in self.unbonding_queue.drain(..) {
            if current_height >= entry.matures_at {
                matured.push((entry.account, entry.amount));
            } else {
                remaining.push(entry);
            }
        }

        self.unbonding_queue = remaining;
        matured
    }

    /// Activate all validators that have been bonding for at least one full epoch,
    /// subject to the `MAX_ACTIVE_VALIDATORS` cap.
    ///
    /// Validators transition from Bonding → Active once their bond has been confirmed
    /// for EPOCH blocks **and** a slot is available. If the cap is already reached,
    /// eligible validators remain in Bonding status until a slot opens via unbond or
    /// slash. No bond transaction is ever rejected — all validators are queued fairly.
    pub fn activate_matured_validators(&mut self, current_height: u64) -> Vec<ObjectId> {
        let mut activated = Vec::new();
        let mut current_active = self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .count();

        for validator in self.validators.values_mut() {
            if validator.status == ValidatorStatus::Bonding {
                if let Some(earliest) = validator.entries.first() {
                    if current_height >= earliest.bonded_at_height.saturating_add(EPOCH) {
                        if current_active >= MAX_ACTIVE_VALIDATORS {
                            // Cap reached — this validator remains in Bonding until a slot opens
                            continue;
                        }
                        validator.status = ValidatorStatus::Active;
                        validator.last_signed_height = current_height;
                        activated.push(validator.object_id.clone());
                        current_active += 1;
                    }
                }
            }
        }
        activated
    }

    /// Count of validators currently in `Active` status.
    pub fn total_active_validators(&self) -> usize {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .count()
    }

    /// Count of validators currently in `Bonding` status (waiting for a slot or for epoch maturity).
    pub fn total_bonding_validators(&self) -> usize {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Bonding)
            .count()
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

    /// Apply graduated slashing for double-signing.
    ///
    /// - **1st offense**: burn 10% of total stake, stay Active
    /// - **2nd offense**: burn 33% of total stake, suspended to Bonding for one epoch
    /// - **3rd+ offense**: burn 100% of total stake, permanent `Slashed` status
    ///
    /// The offense counter resets to zero if more than 10,240 blocks pass without
    /// another slash — a clean record eventually restores full trust. The burn is
    /// proportionally distributed across all bond entries.
    ///
    /// Returns the Flake amount burned. Returns `Ok(0)` if the validator is already
    /// permanently `Slashed` (idempotent for already-punished validators).
    pub fn graduated_slash(&mut self, object_id: &ObjectId, current_height: u64) -> Result<FlakeAmount, String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not found".to_string())?;

        // Already permanently slashed — nothing more to take
        if validator.status == ValidatorStatus::Slashed {
            return Ok(0);
        }

        // Reset offense counter if clean for more than 10,240 blocks
        if validator.last_slash_height > 0
            && current_height.saturating_sub(validator.last_slash_height) > 10_240
        {
            validator.slash_offense_count = 0;
        }

        validator.slash_offense_count = validator.slash_offense_count.saturating_add(1);
        validator.last_slash_height = current_height;

        let total_stake = validator.total_stake();
        let offense = validator.slash_offense_count;

        match offense {
            1 => {
                // 10% burn; validator stays Active
                let burn = total_stake / 10;
                let keep = total_stake - burn;
                scale_entries(&mut validator.entries, keep, total_stake);
                Ok(burn)
            }
            2 => {
                // 33% burn; suspend for one epoch (reset to Bonding)
                let burn = total_stake / 3;
                let keep = total_stake - burn;
                scale_entries(&mut validator.entries, keep, total_stake);
                validator.status = ValidatorStatus::Bonding;
                Ok(burn)
            }
            _ => {
                // 100% burn; permanent Slashed
                let burn = total_stake;
                validator.status = ValidatorStatus::Slashed;
                for entry in &mut validator.entries {
                    entry.stake = 0;
                }
                Ok(burn)
            }
        }
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

    /// Load validators and unbonding queue from serialized data. Used for state restoration.
    pub fn load_from_validators(validators: Vec<ValidatorInfo>, unbonding_queue: Vec<PendingUnbond>) -> Self {
        let mut set = ValidatorSet {
            validators: HashMap::new(),
            unbonding_queue,
        };
        for v in validators {
            set.validators.insert(v.object_id.clone(), v);
        }
        set
    }

    /// Compute a deterministic Blake3-256 state root hash over all validators
    /// and their bond entries. Validators are sorted by ObjectId for determinism.
    /// Also includes the unbonding queue to capture pending stake withdrawals.
    pub fn compute_state_root(&self) -> opolys_core::Hash {
        let mut sorted_ids: Vec<&ObjectId> = self.validators.keys().collect();
        sorted_ids.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));

        let mut hasher = Blake3Hasher::new();

        // Hash all validator state (sorted by ObjectId)
        for id in sorted_ids {
            if let Some(validator) = self.validators.get(id) {
                if let Ok(bytes) = borsh::to_vec(validator) {
                    hasher.update(&bytes);
                }
            }
        }

        // Hash the unbonding queue (order matters — it's FIFO)
        for entry in &self.unbonding_queue {
            if let Ok(bytes) = borsh::to_vec(entry) {
                hasher.update(&bytes);
            }
        }

        hasher.finalize()
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

/// Proportionally scale all bond entries so their total equals `remaining`.
///
/// Integer rounding dust (always < entry count flakes) is added to the last entry.
/// No-ops when `total` is zero (prevents division-by-zero).
fn scale_entries(entries: &mut Vec<BondEntry>, remaining: FlakeAmount, total: FlakeAmount) {
    if total == 0 || entries.is_empty() {
        return;
    }
    let mut distributed: u64 = 0;
    for entry in entries.iter_mut() {
        let share = (entry.stake as u128 * remaining as u128 / total as u128) as u64;
        entry.stake = share;
        distributed += share;
    }
    // Add rounding dust to the last entry to keep total exact
    if distributed < remaining {
        if let Some(last) = entries.last_mut() {
            last.stake += remaining - distributed;
        }
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
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE + MIN_BOND_STAKE / 2, 500).unwrap();

        // Should return all requested amount since total stake > amount
        assert_eq!(unbonded, MIN_BOND_STAKE + MIN_BOND_STAKE / 2);

        let v = vs.get_validator(&id).unwrap();
        // Entry 1 is gone, entry 2 has 1.5 OPL remaining with original timestamp
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE * 2 - MIN_BOND_STAKE / 2);
        // Split entry keeps original timestamp for seniority
        assert_eq!(v.entries[0].bonded_at_timestamp, 1000);

        // The unbonded amount should be in the unbonding queue
        assert_eq!(vs.unbonding_queue.len(), 1);
        assert_eq!(vs.unbonding_queue[0].amount, MIN_BOND_STAKE + MIN_BOND_STAKE / 2);
        assert_eq!(vs.unbonding_queue[0].matures_at, 500 + EPOCH as u64);
    }

    #[test]
    fn unbond_fifo_removes_validator_when_empty() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Unbond the entire stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);
        assert_eq!(vs.validator_count(), 0);
        // Unbonding queue holds the pending entry
        assert_eq!(vs.unbonding_queue.len(), 1);
    }

    #[test]
    fn unbond_more_than_stake() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Try to unbond more than total stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE * 10, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE); // Only unbond what's available
        assert_eq!(vs.validator_count(), 0);
    }

    #[test]
    fn unbond_nonexistent_validator() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        let result = vs.unbond_amount(&id, MIN_BOND_STAKE, 100);
        assert!(result.is_err());
    }

    #[test]
    fn process_matured_unbonds_returns_matured_entries() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 3, 0, 0).unwrap();

        // Unbond at height 100, matures at 100 + 1024 = 1124
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);

        // At height 1123, nothing has matured yet
        let matured = vs.process_matured_unbonds(1123);
        assert!(matured.is_empty());
        assert_eq!(vs.unbonding_queue.len(), 1);

        // At height 1124, the entry matures
        let matured = vs.process_matured_unbonds(1124);
        assert_eq!(matured.len(), 1);
        assert_eq!(matured[0].0, id);
        assert_eq!(matured[0].1, MIN_BOND_STAKE);
        assert!(vs.unbonding_queue.is_empty());
    }

    #[test]
    fn activate_matured_validators_transitions_after_epoch() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // At height 0, validator should be in Bonding status
        assert_eq!(vs.get_validator(&id).unwrap().status, ValidatorStatus::Bonding);

        // Before epoch boundary, no activation
        let activated = vs.activate_matured_validators(1023);
        assert!(activated.is_empty());
        assert_eq!(vs.get_validator(&id).unwrap().status, ValidatorStatus::Bonding);

        // At epoch boundary, validator activates
        let activated = vs.activate_matured_validators(1024);
        assert_eq!(activated.len(), 1);
        assert_eq!(activated[0], id);
        assert_eq!(vs.get_validator(&id).unwrap().status, ValidatorStatus::Active);
    }

    #[test]
    fn validator_cap_holds_excess_in_bonding() {
        // Fill the active set to MAX_ACTIVE_VALIDATORS, then bond one more.
        // The extra validator must remain Bonding even after its epoch matures.
        // This test uses a tiny fake cap: we bond MAX_ACTIVE_VALIDATORS + 1
        // validators all at height 0 and check that exactly MAX_ACTIVE_VALIDATORS
        // become Active at the epoch boundary, with the remainder still Bonding.
        //
        // Testing with the real cap (5,000) is expensive, so we verify the logic
        // with a smaller set and trust that MAX_ACTIVE_VALIDATORS is just a constant.

        let mut vs = ValidatorSet::new();

        // Bond (MAX_ACTIVE_VALIDATORS + 2) validators, all bonded at height 0
        let n = MAX_ACTIVE_VALIDATORS + 2;
        let mut ids = Vec::new();
        for i in 0..n {
            let seed = format!("validator_{}", i);
            let id = test_id(seed.as_bytes());
            vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
            ids.push(id);
        }
        assert_eq!(vs.total_active_validators(), 0);
        assert_eq!(vs.total_bonding_validators(), n);

        // Activate at epoch boundary
        let activated = vs.activate_matured_validators(EPOCH as u64);
        assert_eq!(activated.len(), MAX_ACTIVE_VALIDATORS);
        assert_eq!(vs.total_active_validators(), MAX_ACTIVE_VALIDATORS);
        // The 2 extras remain Bonding — waiting for a slot
        assert_eq!(vs.total_bonding_validators(), 2);

        // A second call at the same height should not activate more (cap still full)
        let activated2 = vs.activate_matured_validators(EPOCH as u64);
        assert!(activated2.is_empty());
        assert_eq!(vs.total_active_validators(), MAX_ACTIVE_VALIDATORS);
    }

    #[test]
    fn graduated_slash_first_offense_burns_ten_percent() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0).unwrap(); // 10 OPL
        vs.activate(&id, 1).unwrap();

        let burned = vs.graduated_slash(&id, 100).unwrap();
        assert_eq!(burned, MIN_BOND_STAKE); // 10% of 10 OPL = 1 OPL
        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 9); // 9 OPL remains
        assert_eq!(v.status, ValidatorStatus::Active); // stays Active
        assert_eq!(v.slash_offense_count, 1);
    }

    #[test]
    fn graduated_slash_second_offense_burns_thirty_three_percent_and_suspends() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0).unwrap(); // 10 OPL
        vs.activate(&id, 1).unwrap();

        vs.graduated_slash(&id, 100).unwrap(); // 1st: burn 1 OPL, keep 9 OPL
        let burned = vs.graduated_slash(&id, 200).unwrap(); // 2nd: burn 33% of 9 OPL
        assert_eq!(burned, MIN_BOND_STAKE * 3); // floor(9,000,000 / 3) = 3 OPL
        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 6); // 6 OPL remains
        assert_eq!(v.status, ValidatorStatus::Bonding); // suspended
        assert_eq!(v.slash_offense_count, 2);
    }

    #[test]
    fn graduated_slash_third_offense_burns_all_and_permanently_slashes() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap(); // total: 3 OPL
        vs.activate(&id, 1).unwrap();

        vs.graduated_slash(&id, 100).unwrap(); // 1st
        vs.graduated_slash(&id, 200).unwrap(); // 2nd
        let burned = vs.graduated_slash(&id, 300).unwrap(); // 3rd: 100% burn

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.total_stake(), 0);
        assert_eq!(v.status, ValidatorStatus::Slashed);
        assert!(burned > 0);
        for entry in &v.entries {
            assert_eq!(entry.stake, 0);
        }
    }

    #[test]
    fn graduated_slash_already_slashed_returns_zero() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.graduated_slash(&id, 100).unwrap();
        vs.graduated_slash(&id, 200).unwrap();
        vs.graduated_slash(&id, 300).unwrap(); // permanently slashed

        let burned = vs.graduated_slash(&id, 400).unwrap(); // idempotent
        assert_eq!(burned, 0);
    }

    #[test]
    fn graduated_slash_counter_resets_after_clean_period() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.graduated_slash(&id, 100).unwrap(); // 1st offense at height 100
        assert_eq!(vs.get_validator(&id).unwrap().slash_offense_count, 1);

        // Clean for > 10,240 blocks → counter resets
        vs.graduated_slash(&id, 100 + 10_241).unwrap(); // treated as 1st offense again
        assert_eq!(vs.get_validator(&id).unwrap().slash_offense_count, 1);
        assert_eq!(vs.get_validator(&id).unwrap().status, ValidatorStatus::Active);
    }

    #[test]
    fn rebond_after_permanent_slash_rejected() {
        let mut vs = ValidatorSet::new();
        let id = test_id(b"validator1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.graduated_slash(&id, 100).unwrap();
        vs.graduated_slash(&id, 200).unwrap();
        vs.graduated_slash(&id, 300).unwrap(); // permanently slashed

        let result = vs.bond(id.clone(), MIN_BOND_STAKE, 400, 400);
        assert!(result.is_err(), "Slashed validators must not be able to re-bond");
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
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE * 2, 500).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE * 2);

        let v = vs.get_validator(&id).unwrap();
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE * 2); // 3 - 1 = 2
        assert_eq!(v.entries[0].bonded_at_timestamp, 2000); // Keeps original timestamp
    }

    /// Full lifecycle integration test: bond → activate → unbond → mature → slash
    #[test]
    fn validator_full_lifecycle() {
        let mut vs = ValidatorSet::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        let charlie = test_id(b"charlie");

        // Phase 1: Three validators bond at height 0
        vs.bond(alice.clone(), MIN_BOND_STAKE * 10, 0, 0).unwrap(); // Alice: 10 OPL
        vs.bond(bob.clone(), MIN_BOND_STAKE * 20, 0, 0).unwrap();   // Bob: 20 OPL
        vs.bond(charlie.clone(), MIN_BOND_STAKE * 5, 0, 0).unwrap(); // Charlie: 5 OPL

        // All start as Bonding
        assert_eq!(vs.get_validator(&alice).unwrap().status, ValidatorStatus::Bonding);
        assert_eq!(vs.get_validator(&bob).unwrap().status, ValidatorStatus::Bonding);
        assert_eq!(vs.get_validator(&charlie).unwrap().status, ValidatorStatus::Bonding);
        assert_eq!(vs.total_bonded_stake(), MIN_BOND_STAKE * 35);

        // Phase 2: Before epoch boundary, no activation
        let activated = vs.activate_matured_validators(1023);
        assert!(activated.is_empty());

        // Phase 3: At epoch boundary (height 1024), all validators activate
        let activated = vs.activate_matured_validators(1024);
        assert_eq!(activated.len(), 3);
        assert_eq!(vs.get_validator(&alice).unwrap().status, ValidatorStatus::Active);
        assert_eq!(vs.get_validator(&bob).unwrap().status, ValidatorStatus::Active);
        assert_eq!(vs.get_validator(&charlie).unwrap().status, ValidatorStatus::Active);

        // Phase 4: Block producer selection — deterministic via seed
        let producer = vs.select_block_producer(0, 42).unwrap();
        // Bob has 2x the stake of Alice, so Bob should be selected more often
        // but Bob is not guaranteed — just verify selection works
        assert!(
            producer.object_id == alice || producer.object_id == bob || producer.object_id == charlie,
            "Producer must be one of the bonded validators"
        );

        // Phase 5: Unbond Alice at height 2000, matures at height 2000 + 1024 = 3024
        let unbonded = vs.unbond_amount(&alice, MIN_BOND_STAKE * 3, 2000).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE * 3);
        assert_eq!(vs.unbonding_queue.len(), 1);
        assert_eq!(vs.unbonding_queue[0].account, alice);
        assert_eq!(vs.unbonding_queue[0].amount, MIN_BOND_STAKE * 3);
        assert_eq!(vs.unbonding_queue[0].matures_at, 2000 + EPOCH as u64);

        // Alice still has 7 OPL bonded
        assert_eq!(vs.get_validator(&alice).unwrap().total_stake(), MIN_BOND_STAKE * 7);

        // Phase 6: Process matured unbonds — nothing at height 3023
        let matured = vs.process_matured_unbonds(3023);
        assert!(matured.is_empty());

        // At height 3024, Alice's unbonding entry matures
        let matured = vs.process_matured_unbonds(3024);
        assert_eq!(matured.len(), 1);
        assert_eq!(matured[0].0, alice);
        assert_eq!(matured[0].1, MIN_BOND_STAKE * 3);
        assert!(vs.unbonding_queue.is_empty());

        // Phase 7: Charlie triple-offends — graduated slashing to permanent Slashed.
        assert_eq!(vs.get_validator(&charlie).unwrap().status, ValidatorStatus::Active);

        // 1st offense: 10% burn of 5 OPL = 0.5 OPL, stays Active
        let burn1 = vs.graduated_slash(&charlie, 2100).unwrap();
        assert_eq!(burn1, MIN_BOND_STAKE / 2);
        assert_eq!(vs.get_validator(&charlie).unwrap().status, ValidatorStatus::Active);

        // 2nd offense: 33% burn of 4.5 OPL = 1.5 OPL, suspended to Bonding
        let burn2 = vs.graduated_slash(&charlie, 2200).unwrap();
        assert_eq!(burn2, MIN_BOND_STAKE + MIN_BOND_STAKE / 2);
        assert_eq!(vs.get_validator(&charlie).unwrap().status, ValidatorStatus::Bonding);

        // 3rd offense: 100% burn of remaining 3 OPL, permanent Slashed
        let burn3 = vs.graduated_slash(&charlie, 2300).unwrap();
        assert_eq!(burn3, MIN_BOND_STAKE * 3);
        assert_eq!(vs.get_validator(&charlie).unwrap().status, ValidatorStatus::Slashed);
        assert_eq!(vs.get_validator(&charlie).unwrap().total_stake(), 0);

        // Total burned across 3 offenses = 0.5 + 1.5 + 3 = 5 OPL (original stake)
        assert_eq!(burn1 + burn2 + burn3, MIN_BOND_STAKE * 5);

        // Slashed validator excluded from Active set
        let active_count = vs.active_validators().len();
        assert_eq!(active_count, 2); // Only Alice and Bob remain active

        // Total bonded stake excludes permanently slashed validators
        let bonded = vs.total_bonded_stake();
        assert_eq!(bonded, MIN_BOND_STAKE * 7 + MIN_BOND_STAKE * 20); // Alice 7 + Bob 20
    }
}