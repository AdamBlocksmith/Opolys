//! # Proof-of-stake refiner management for Opolys.
//!
//! Opolys refiners bond stake as collateral and earn block rewards
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
//! **Unbonding is FIFO** — when a refiner unbonds, the oldest entries are
//! consumed first. If the unbond amount exceeds an entry's stake, that entry
//! is fully consumed and the remainder comes from the next oldest. Split
//! entries keep their original `bonded_at_timestamp` for weight calculation.
//! The 1 OPL minimum only applies to new bond entries, not to residuals from
//! FIFO splits.
//!
//! **Slashing is narrowly scoped to double-signing only.** No governance
//! body can slash for other reasons. A slashed refiner's entire stake across
//! all entries is burned (not confiscated to any treasury), permanently
//! removing it from circulation.
//!
//! Block producers are selected via weighted random sampling, where the seed
//! is derived from on-chain entropy. There are no rounds, no schedules, and
//! no fixed refiner sets — just continuous weighted selection.

use borsh::{BorshDeserialize, BorshSerialize};
use opolys_core::{
    ANNUAL_ATTRITION_PERMILLE, EPOCH, FlakeAmount, MAX_ACTIVE_REFINERS, MIN_BOND_STAKE, ObjectId,
    RefinerStatus,
};
use opolys_crypto::{Blake3Hasher, DOMAIN_STATE_ROOT};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A pending unbonding entry that matures after a delay of EPOCH blocks.
///
/// When a refiner unbonds, the stake is not returned immediately. Instead,
/// it enters the unbonding queue and matures after `UNBONDING_DELAY_BLOCKS`
/// (960 blocks = one epoch). Once matured, the stake is returned to the
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

/// A single bond entry within a refiner's stake.
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
        crate::emission::compute_refiner_weight(self.stake, self.age_years_milli(current_timestamp))
    }
}

/// Information about a bonded refiner.
///
/// Refiners hold one or more bond entries, each with its own stake amount
/// and seniority clock. The total weight is the sum of per-entry weights,
/// giving a logarithmic seniority bonus that diminishes over time.
///
/// Double-signing triggers graduated slashing: 10% burn on first offense,
/// 33% burn + suspension on second, 100% burn + permanent Slashed on third+.
/// Slashed stake is removed from circulation, not transferred to any treasury.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct RefinerInfo {
    /// The refiner's on-chain identity (Blake3 hash of public key).
    pub object_id: ObjectId,
    /// The refiner's bond entries, each with its own stake and seniority.
    /// Entries are sorted by `bonded_at_timestamp` (FIFO order).
    pub entries: Vec<BondEntry>,
    /// Current lifecycle status: Bonding → Active → (Slashed | Unbonded).
    pub status: RefinerStatus,
    /// Height of the most recent block this refiner signed, updated on
    /// each signature to track liveness.
    pub last_signed_height: u64,
    /// Consecutive valid block attestations included on-chain for this refiner.
    /// Reward weighting is introduced later; for now this records verified liveness.
    pub consecutive_correct_attestations: u64,
}

impl RefinerInfo {
    /// Create a new refiner with their first bond entry.
    fn new(object_id: ObjectId, stake: FlakeAmount, height: u64, timestamp: u64) -> Self {
        RefinerInfo {
            object_id,
            entries: vec![BondEntry {
                stake,
                bonded_at_height: height,
                bonded_at_timestamp: timestamp,
            }],
            status: RefinerStatus::Bonding,
            last_signed_height: 0,
            consecutive_correct_attestations: 0,
        }
    }

    /// Total stake across all bond entries.
    pub fn total_stake(&self) -> FlakeAmount {
        self.entries.iter().map(|e| e.stake).sum()
    }

    /// Compute the refiner's total weight as the sum of per-entry weights.
    ///
    /// Each entry's weight is `stake × (1 + ln(1 + age_years))`, giving
    /// a logarithmic seniority bonus that diminishes over time.
    pub fn weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.entries
            .iter()
            .map(|e| e.weight(current_timestamp))
            .sum()
    }

    /// Add a new bond entry (top-up) to this refiner.
    ///
    /// Each new entry must meet the `MIN_BOND_STAKE` minimum (1 OPL).
    /// The entry gets its own seniority clock starting from zero.
    /// If an entry with the same `bonded_at_timestamp` already exists,
    /// stakes are merged (auto-merge) to reduce entry count.
    fn add_entry(&mut self, stake: FlakeAmount, height: u64, timestamp: u64) {
        // Auto-merge: if an entry with the same timestamp exists, combine stakes
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.bonded_at_timestamp == timestamp)
        {
            existing.stake = existing.stake.saturating_add(stake);
            return;
        }
        // Otherwise, insert in sorted order by timestamp (FIFO)
        let entry = BondEntry {
            stake,
            bonded_at_height: height,
            bonded_at_timestamp: timestamp,
        };
        let pos = self
            .entries
            .iter()
            .position(|e| e.bonded_at_timestamp > timestamp)
            .unwrap_or(self.entries.len());
        self.entries.insert(pos, entry);
    }

    /// Unbond `amount` Flakes from this refiner using FIFO order.
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

/// Maximum number of refiners kept in the in-memory cache.
/// Active refiners always stay cached; others are evicted when the cache
/// is full. Non-cached refiners are loaded from RocksDB on demand.
const REFINER_CACHE_MAX_SIZE: usize = 10_000;

/// The set of all bonded refiners, supporting bonding, unbonding,
/// activating, slashing, and weighted block-producer selection.
///
/// Only **double-signing** triggers slashing in Opolys — no other offense
/// results in stake removal. Slashed stake is burned (removed from supply),
/// not sent to any entity.
///
/// Supports up to 524,288 total refiners with a 5,000-slot active set.
/// Refiners outside the top-5,000 by weight sit in Waiting status.
/// rerank_refiners() at epoch boundaries promotes/demotes as weights shift.
#[derive(Debug)]
pub struct RefinerSet {
    /// In-memory refiner cache (active set always resident; others evicted when full).
    cached_refiners: HashMap<ObjectId, RefinerInfo>,
    /// In-memory active set for O(1) total_bonded_stake() and active_refiners().
    active_set: Vec<ObjectId>,
    /// Refiners modified since the last state root was computed.
    /// Used for incremental state root computation (O(changed) not O(total)).
    pub dirty_refiners: HashSet<ObjectId>,
    /// Pending unbonding entries that mature after EPOCH blocks.
    pub unbonding_queue: Vec<PendingUnbond>,
}

impl RefinerSet {
    /// Create an empty refiner set.
    pub fn new() -> Self {
        RefinerSet {
            cached_refiners: HashMap::new(),
            active_set: Vec::new(),
            dirty_refiners: HashSet::new(),
            unbonding_queue: Vec::new(),
        }
    }

    /// Evict non-active refiners from the in-memory cache when it exceeds
    /// `REFINER_CACHE_MAX_SIZE`. Active refiners are never evicted.
    fn evict_cache_if_full(&mut self) {
        if self.cached_refiners.len() <= REFINER_CACHE_MAX_SIZE {
            return;
        }
        let evict: Vec<ObjectId> = self
            .cached_refiners
            .iter()
            .filter(|(_, v)| {
                v.status != RefinerStatus::Active && v.status != RefinerStatus::Waiting
            })
            .map(|(id, _)| id.clone())
            .take(
                self.cached_refiners
                    .len()
                    .saturating_sub(REFINER_CACHE_MAX_SIZE),
            )
            .collect();
        for id in evict {
            self.cached_refiners.remove(&id);
        }
    }

    /// Clear the dirty set (called after state root is committed).
    pub fn clear_dirty(&mut self) {
        self.dirty_refiners.clear();
    }

    /// Bond stake as a refiner entry. If the refiner doesn't exist, creates
    /// a new refiner with this as their first entry (status: Bonding). If the
    /// refiner already exists, adds to the existing entry (auto-merge) if same
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

        if let Some(refiner) = self.cached_refiners.get_mut(&object_id) {
            if refiner.status == RefinerStatus::Slashed {
                return Err("Slashed refiners cannot re-bond".to_string());
            }
            refiner.add_entry(stake, height, timestamp);
            self.dirty_refiners.insert(object_id);
        } else {
            // New refiner: create with first bond entry
            let id = object_id.clone();
            self.cached_refiners.insert(
                object_id.clone(),
                RefinerInfo::new(object_id, stake, height, timestamp),
            );
            self.dirty_refiners.insert(id);
            self.evict_cache_if_full();
        }

        Ok(())
    }

    /// Unbond `amount` Flakes from a refiner using FIFO order.
    ///
    /// The oldest entries are consumed first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the
    /// next oldest. The unbonded stake enters the unbonding queue and will
    /// mature after `UNBONDING_DELAY_BLOCKS` (960 blocks = one epoch).
    ///
    /// If the refiner has no remaining entries after unbonding, they are
    /// removed from the refiner set but not slashed.
    pub fn unbond_amount(
        &mut self,
        object_id: &ObjectId,
        amount: FlakeAmount,
        current_height: u64,
    ) -> Result<FlakeAmount, String> {
        let refiner = self
            .cached_refiners
            .get_mut(object_id)
            .ok_or_else(|| "Refiner not bonded".to_string())?;

        let total_stake = refiner.total_stake();
        if total_stake == 0 {
            return Err("Refiner has no stake".to_string());
        }

        // Unbond up to the amount available
        let actual_amount = amount.min(total_stake);
        let unbonded = refiner.unbond_fifo(actual_amount);

        // Enqueue the unbonding entry with a maturation height
        let matures_at = current_height.saturating_add(EPOCH);
        self.unbonding_queue.push(PendingUnbond {
            account: object_id.clone(),
            amount: unbonded,
            matures_at,
        });

        // If no entries remain, remove the refiner entirely
        if refiner.entries.is_empty() {
            self.cached_refiners.remove(object_id);
            self.active_set.retain(|id| id != object_id);
        }
        self.dirty_refiners.insert(object_id.clone());

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

    /// Activate all refiners that have been bonding for at least one full epoch.
    ///
    /// Moves Bonding → Waiting. rerank_refiners() handles the Waiting → Active
    /// promotion (top-N by weight). This separation allows fair competition for
    /// active slots: all epoch-matured refiners become eligible, then the highest-
    /// weight ones are promoted.
    pub fn activate_matured_refiners(&mut self, current_height: u64) -> Vec<ObjectId> {
        let mut newly_waiting = Vec::new();

        // Collect eligible IDs first to avoid borrow conflict
        let eligible: Vec<ObjectId> = self
            .cached_refiners
            .iter()
            .filter(|(_, v)| {
                v.status == RefinerStatus::Bonding
                    && v.entries.first().map_or(false, |e| {
                        current_height >= e.bonded_at_height.saturating_add(EPOCH)
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in eligible {
            if let Some(v) = self.cached_refiners.get_mut(&id) {
                v.status = RefinerStatus::Waiting;
                v.last_signed_height = current_height;
                self.dirty_refiners.insert(id.clone());
                newly_waiting.push(id);
            }
        }
        newly_waiting
    }

    /// Re-rank all eligible refiners at an epoch boundary.
    ///
    /// Collects all non-Slashed refiners with stake > 0, sorts by total_weight()
    /// descending, promotes the top MAX_ACTIVE_REFINERS to Active, and demotes the
    /// rest to Waiting. Returns (newly_activated, newly_demoted) for logging.
    ///
    /// Must be called at epoch boundaries (height % EPOCH == 0) after
    /// activate_matured_refiners().
    pub fn rerank_refiners(&mut self, current_timestamp: u64) -> (Vec<ObjectId>, Vec<ObjectId>) {
        // Sort all eligible refiners by weight descending
        let mut eligible: Vec<(ObjectId, u64)> = self
            .cached_refiners
            .iter()
            .filter(|(_, v)| v.status != RefinerStatus::Slashed && v.total_stake() > 0)
            .map(|(id, v)| (id.clone(), v.weight(current_timestamp)))
            .collect();
        eligible.sort_by(|a, b| b.1.cmp(&a.1));

        let mut newly_activated = Vec::new();
        let mut newly_demoted = Vec::new();

        for (i, (id, _)) in eligible.iter().enumerate() {
            if let Some(v) = self.cached_refiners.get_mut(id) {
                if i < MAX_ACTIVE_REFINERS {
                    if v.status == RefinerStatus::Waiting {
                        v.status = RefinerStatus::Active;
                        self.active_set.push(id.clone());
                        self.dirty_refiners.insert(id.clone());
                        newly_activated.push(id.clone());
                    }
                } else {
                    if v.status == RefinerStatus::Active {
                        v.status = RefinerStatus::Waiting;
                        self.active_set.retain(|a| a != id);
                        self.dirty_refiners.insert(id.clone());
                        newly_demoted.push(id.clone());
                    }
                }
            }
        }

        (newly_activated, newly_demoted)
    }

    /// Apply annual stake decay to all non-slashed refiners.
    ///
    /// Mirrors gold vault storage fees: ~1.5% of bonded stake decays per year.
    /// Applied once per epoch (960 blocks = 24 hours).
    /// Per-epoch decay factor: (1 - ANNUAL_ATTRITION_PERMILLE/1000)^(1/365) ≈ 999_959/1_000_000.
    ///
    /// Returns the total amount of stake burned across all refiners.
    pub fn decay_stake(&mut self) -> FlakeAmount {
        // Per-epoch decay: (1 - 0.015)^(1/365) ≈ 0.999959 per epoch
        // Each entry: stake = stake * DECAY_NUMERATOR / DECAY_DENOMINATOR
        // where DECAY_NUMERATOR ≈ 999_959, DECAY_DENOMINATOR = 1_000_000
        const DECAY_NUMERATOR: u64 = 1_000_000 - (ANNUAL_ATTRITION_PERMILLE * 1_000 / 365);
        const DECAY_DENOMINATOR: u64 = 1_000_000;
        let mut total_burned: FlakeAmount = 0;
        for refiner in self.cached_refiners.values_mut() {
            if refiner.status == RefinerStatus::Slashed {
                continue;
            }
            for entry in &mut refiner.entries {
                if entry.stake == 0 {
                    continue;
                }
                let new_stake = ((entry.stake as u128 * DECAY_NUMERATOR as u128)
                    / DECAY_DENOMINATOR as u128) as FlakeAmount;
                let burned = entry.stake.saturating_sub(new_stake);
                total_burned = total_burned.saturating_add(burned);
                entry.stake = new_stake;
            }
        }
        total_burned
    }

    /// Count of refiners currently in `Active` status.
    pub fn total_active_refiners(&self) -> usize {
        self.active_set.len()
    }

    /// Count of refiners currently in `Bonding` status (waiting for epoch maturity).
    pub fn total_bonding_refiners(&self) -> usize {
        self.cached_refiners
            .values()
            .filter(|v| v.status == RefinerStatus::Bonding)
            .count()
    }

    /// Count of refiners currently in `Waiting` status (eligible but outside top-N).
    pub fn total_waiting_refiners(&self) -> usize {
        self.cached_refiners
            .values()
            .filter(|v| v.status == RefinerStatus::Waiting)
            .count()
    }

    /// Directly activate a refiner (test helper and slash suspension recovery).
    /// Transitions Bonding → Active and maintains active_set.
    pub fn activate(&mut self, object_id: &ObjectId, height: u64) -> Result<(), String> {
        let refiner = self
            .cached_refiners
            .get_mut(object_id)
            .ok_or_else(|| "Refiner not found".to_string())?;
        if refiner.status != RefinerStatus::Bonding {
            return Err("Refiner not in bonding state".to_string());
        }
        refiner.status = RefinerStatus::Active;
        refiner.last_signed_height = height;
        if !self.active_set.contains(object_id) {
            self.active_set.push(object_id.clone());
        }
        self.dirty_refiners.insert(object_id.clone());
        Ok(())
    }

    /// Slash a refiner for double-signing — 100% burn, permanent removal.
    ///
    /// Any double-sign proves the refiner's key was used to sign two different
    /// blocks at the same height. The entire stake is burned (no recovery),
    /// and the refiner is permanently set to `Slashed` status.
    ///
    /// Returns the Flake amount burned. Returns `Ok(0)` if the refiner is already
    /// `Slashed` (idempotent for already-punished refiners).
    pub fn slash_refiner(
        &mut self,
        object_id: &ObjectId,
        _current_height: u64,
    ) -> Result<FlakeAmount, String> {
        let refiner = self
            .cached_refiners
            .get_mut(object_id)
            .ok_or_else(|| "Refiner not found".to_string())?;

        // Already permanently slashed — nothing more to take
        if refiner.status == RefinerStatus::Slashed {
            return Ok(0);
        }

        // 100% burn; permanent Slashed status
        let burn = refiner.total_stake();
        refiner.status = RefinerStatus::Slashed;
        for entry in &mut refiner.entries {
            entry.stake = 0;
        }
        self.active_set.retain(|id| id != object_id);
        self.dirty_refiners.insert(object_id.clone());
        Ok(burn)
    }

    /// Look up a refiner by their ObjectId.
    pub fn get_refiner(&self, object_id: &ObjectId) -> Option<&RefinerInfo> {
        self.cached_refiners.get(object_id)
    }

    /// Record one verified on-chain attestation for an active refiner.
    pub fn record_correct_attestation(&mut self, object_id: &ObjectId) -> Result<u64, String> {
        let refiner = self
            .cached_refiners
            .get_mut(object_id)
            .ok_or_else(|| "Attestation refiner not found".to_string())?;
        if refiner.status != RefinerStatus::Active {
            return Err("Attestation refiner is not active".to_string());
        }
        refiner.consecutive_correct_attestations =
            refiner.consecutive_correct_attestations.saturating_add(1);
        self.dirty_refiners.insert(object_id.clone());
        Ok(refiner.consecutive_correct_attestations)
    }

    /// Total stake across all Bonding, Waiting, and Active refiners. Used to
    /// compute stake coverage, which determines the PoW/refiner reward split.
    pub fn total_bonded_stake(&self) -> FlakeAmount {
        // Use active_set for Active refiners (O(active_set.len()))
        let active_stake: FlakeAmount = self
            .active_set
            .iter()
            .filter_map(|id| self.cached_refiners.get(id))
            .map(|v| v.total_stake())
            .sum();
        // Add Bonding and Waiting stake (not in active_set)
        let other_stake: FlakeAmount = self
            .cached_refiners
            .values()
            .filter(|v| v.status == RefinerStatus::Bonding || v.status == RefinerStatus::Waiting)
            .map(|v| v.total_stake())
            .sum();
        active_stake.saturating_add(other_stake)
    }

    /// Return all refiners currently in `Active` status.
    pub fn active_refiners(&self) -> Vec<&RefinerInfo> {
        self.active_set
            .iter()
            .filter_map(|id| self.cached_refiners.get(id))
            .collect()
    }

    /// Compute the total weight of all active refiners.
    pub fn total_weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.active_set
            .iter()
            .filter_map(|id| self.cached_refiners.get(id))
            .map(|v| v.weight(current_timestamp))
            .sum()
    }

    /// Number of refiners in the cache (may be less than total if some are on disk).
    pub fn refiner_count(&self) -> usize {
        self.cached_refiners.len()
    }

    /// Return all refiners as a serializable Vec. Used for persistence.
    pub fn all_refiners(&self) -> Vec<RefinerInfo> {
        self.cached_refiners.values().cloned().collect()
    }

    /// Return the active set IDs.
    pub fn active_set_ids(&self) -> &Vec<ObjectId> {
        &self.active_set
    }

    /// Load refiners and unbonding queue from serialized data. Used for state restoration.
    pub fn load_from_refiners(
        refiners: Vec<RefinerInfo>,
        unbonding_queue: Vec<PendingUnbond>,
    ) -> Self {
        let mut set = RefinerSet {
            cached_refiners: HashMap::new(),
            active_set: Vec::new(),
            dirty_refiners: HashSet::new(),
            unbonding_queue,
        };
        for v in refiners {
            if v.status == RefinerStatus::Active {
                set.active_set.push(v.object_id.clone());
            }
            set.cached_refiners.insert(v.object_id.clone(), v);
        }
        set
    }

    /// Compute a deterministic Blake3-256 state root hash over all refiners
    /// and their bond entries. Refiners are sorted by ObjectId for determinism.
    /// Also includes the unbonding queue to capture pending stake withdrawals.
    pub fn compute_state_root(&self) -> opolys_core::Hash {
        let mut sorted_ids: Vec<&ObjectId> = self.cached_refiners.keys().collect();
        sorted_ids.sort_by(|a, b| a.0.0.cmp(&b.0.0));

        let mut hasher = Blake3Hasher::new();
        hasher.update(DOMAIN_STATE_ROOT);
        hasher.update(b"refiners");

        // Hash all refiner state (sorted by ObjectId)
        for id in sorted_ids {
            if let Some(refiner) = self.cached_refiners.get(id) {
                let bytes = borsh::to_vec(refiner)
                    .expect("Refiner serialization must not fail; this is a consensus bug");
                hasher.update(&bytes);
            }
        }

        // Hash the unbonding queue (order matters — it's FIFO)
        for entry in &self.unbonding_queue {
            let bytes = borsh::to_vec(entry)
                .expect("Unbonding entry serialization must not fail; this is a consensus bug");
            hasher.update(&bytes);
        }

        hasher.finalize()
    }

    /// Select the next block producer via weighted random sampling.
    ///
    /// Each active refiner's total weight (sum of per-entry weights)
    /// determines their probability of being selected. The `seed` parameter
    /// provides on-chain entropy to make the selection deterministic and
    /// verifiable. Returns `None` if there are no active refiners.
    pub fn select_block_producer(&self, current_timestamp: u64, seed: u64) -> Option<&RefinerInfo> {
        let active: Vec<&RefinerInfo> = self
            .active_set
            .iter()
            .filter_map(|id| self.cached_refiners.get(id))
            .collect();

        if active.is_empty() {
            return None;
        }

        let total_weight: FlakeAmount = active.iter().map(|v| v.weight(current_timestamp)).sum();

        if total_weight == 0 {
            return None;
        }

        // Cumulative weighted selection: walk through refiners, accumulating
        // weight until the cumulative total exceeds the random target.
        let mut cumulative = 0u64;
        let target = seed % total_weight;
        for v in &active {
            cumulative += v.weight(current_timestamp);
            if cumulative > target {
                return Some(*v);
            }
        }

        // Fallback: if no refiner was selected due to rounding, pick the last active one.
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
    fn bond_new_refiner() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        assert_eq!(vs.refiner_count(), 1);
        assert_eq!(vs.get_refiner(&id).unwrap().total_stake(), MIN_BOND_STAKE);
        assert_eq!(vs.get_refiner(&id).unwrap().entries.len(), 1);
    }

    #[test]
    fn bond_insufficient_stake_per_entry() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        // MIN_BOND_STAKE is now 1 OPL = 1,000,000 flakes
        assert!(vs.bond(id, 100, 0, 0).is_err());
    }

    #[test]
    fn top_up_bond_adds_entry() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up: add a second entry at a different timestamp
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap();

        let v = vs.get_refiner(&id).unwrap();
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
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 100).unwrap();

        // Top-up at same timestamp — should auto-merge
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 5, 100).unwrap();

        let v = vs.get_refiner(&id).unwrap();
        assert_eq!(v.entries.len(), 1); // Merged, not two entries
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);
    }

    #[test]
    fn unbond_fifo_consumes_oldest_first() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap(); // Entry 1: 1 OPL at t=0
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000).unwrap(); // Entry 2: 2 OPL at t=1000

        // Unbond 1.5 OPL — should fully consume entry 1 (1 OPL) and 0.5 OPL from entry 2
        let unbonded = vs
            .unbond_amount(&id, MIN_BOND_STAKE + MIN_BOND_STAKE / 2, 500)
            .unwrap();

        // Should return all requested amount since total stake > amount
        assert_eq!(unbonded, MIN_BOND_STAKE + MIN_BOND_STAKE / 2);

        let v = vs.get_refiner(&id).unwrap();
        // Entry 1 is gone, entry 2 has 1.5 OPL remaining with original timestamp
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE * 2 - MIN_BOND_STAKE / 2);
        // Split entry keeps original timestamp for seniority
        assert_eq!(v.entries[0].bonded_at_timestamp, 1000);

        // The unbonded amount should be in the unbonding queue
        assert_eq!(vs.unbonding_queue.len(), 1);
        assert_eq!(
            vs.unbonding_queue[0].amount,
            MIN_BOND_STAKE + MIN_BOND_STAKE / 2
        );
        assert_eq!(vs.unbonding_queue[0].matures_at, 500 + EPOCH as u64);
    }

    #[test]
    fn unbond_fifo_removes_refiner_when_empty() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Unbond the entire stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);
        assert_eq!(vs.refiner_count(), 0);
        // Unbonding queue holds the pending entry
        assert_eq!(vs.unbonding_queue.len(), 1);
    }

    #[test]
    fn unbond_more_than_stake() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // Try to unbond more than total stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE * 10, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE); // Only unbond what's available
        assert_eq!(vs.refiner_count(), 0);
    }

    #[test]
    fn unbond_nonexistent_refiner() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        let result = vs.unbond_amount(&id, MIN_BOND_STAKE, 100);
        assert!(result.is_err());
    }

    #[test]
    fn process_matured_unbonds_returns_matured_entries() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 3, 0, 0).unwrap();

        // Unbond at height 100, matures at 100 + EPOCH
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);

        // One block before maturity — nothing matured yet
        let matured = vs.process_matured_unbonds(100 + EPOCH - 1);
        assert!(matured.is_empty());
        assert_eq!(vs.unbonding_queue.len(), 1);

        // At maturity height, the entry matures
        let matured = vs.process_matured_unbonds(100 + EPOCH);
        assert_eq!(matured.len(), 1);
        assert_eq!(matured[0].0, id);
        assert_eq!(matured[0].1, MIN_BOND_STAKE);
        assert!(vs.unbonding_queue.is_empty());
    }

    #[test]
    fn activate_matured_refiners_transitions_after_epoch() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();

        // At height 0, refiner should be in Bonding status
        assert_eq!(vs.get_refiner(&id).unwrap().status, RefinerStatus::Bonding);

        // One block before epoch boundary — no activation yet
        let waiting = vs.activate_matured_refiners(EPOCH - 1);
        assert!(waiting.is_empty());
        assert_eq!(vs.get_refiner(&id).unwrap().status, RefinerStatus::Bonding);

        // At epoch boundary, refiner moves to Waiting
        let waiting = vs.activate_matured_refiners(EPOCH);
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0], id);
        assert_eq!(vs.get_refiner(&id).unwrap().status, RefinerStatus::Waiting);

        // rerank promotes to Active (only 1 refiner, so it takes the slot)
        let (activated, demoted) = vs.rerank_refiners(0);
        assert_eq!(activated.len(), 1);
        assert!(demoted.is_empty());
        assert_eq!(vs.get_refiner(&id).unwrap().status, RefinerStatus::Active);
    }

    #[test]
    fn refiner_cap_holds_excess_in_waiting() {
        // Bond MAX_ACTIVE_REFINERS + 2 refiners. After activate_matured_refiners
        // all move to Waiting. After rerank_refiners, exactly MAX_ACTIVE_REFINERS
        // become Active and 2 remain Waiting.

        let mut vs = RefinerSet::new();

        let n = MAX_ACTIVE_REFINERS + 2;
        let mut ids = Vec::new();
        for i in 0..n {
            let seed = format!("refiner_{}", i);
            let id = test_id(seed.as_bytes());
            vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
            ids.push(id);
        }
        assert_eq!(vs.total_active_refiners(), 0);
        assert_eq!(vs.total_bonding_refiners(), n);

        // All move to Waiting at epoch boundary
        let waiting = vs.activate_matured_refiners(EPOCH);
        assert_eq!(waiting.len(), n);
        assert_eq!(vs.total_active_refiners(), 0);
        assert_eq!(vs.total_waiting_refiners(), n);

        // rerank promotes top MAX_ACTIVE_REFINERS to Active
        let (activated, demoted) = vs.rerank_refiners(0);
        assert_eq!(activated.len(), MAX_ACTIVE_REFINERS);
        assert!(demoted.is_empty());
        assert_eq!(vs.total_active_refiners(), MAX_ACTIVE_REFINERS);
        assert_eq!(vs.total_waiting_refiners(), 2);

        // A second rerank at the same height — no change (cap still full)
        let (activated2, demoted2) = vs.rerank_refiners(0);
        assert!(activated2.is_empty());
        assert!(demoted2.is_empty());
        assert_eq!(vs.total_active_refiners(), MAX_ACTIVE_REFINERS);
    }

    #[test]
    fn slash_refiner_burns_all_stake() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0).unwrap(); // 10 OPL
        vs.activate(&id, 1).unwrap();

        let burned = vs.slash_refiner(&id, 100).unwrap();
        assert_eq!(burned, MIN_BOND_STAKE * 10); // 100% burned
        let v = vs.get_refiner(&id).unwrap();
        assert_eq!(v.total_stake(), 0);
        assert_eq!(v.status, RefinerStatus::Slashed);
    }

    #[test]
    fn slash_refiner_already_slashed_returns_zero() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.slash_refiner(&id, 100).unwrap(); // permanently slashed

        let burned = vs.slash_refiner(&id, 200).unwrap(); // idempotent
        assert_eq!(burned, 0);
    }

    #[test]
    fn record_correct_attestation_increments_active_refiner() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"attester");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        assert_eq!(vs.record_correct_attestation(&id).unwrap(), 1);
        assert_eq!(vs.record_correct_attestation(&id).unwrap(), 2);
        assert!(vs.dirty_refiners.contains(&id));
        assert_eq!(
            vs.get_refiner(&id)
                .unwrap()
                .consecutive_correct_attestations,
            2
        );
    }

    #[test]
    fn rebond_after_permanent_slash_rejected() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.slash_refiner(&id, 100).unwrap(); // slashed

        let result = vs.bond(id.clone(), MIN_BOND_STAKE, 400, 400);
        assert!(
            result.is_err(),
            "Slashed refiners must not be able to re-bond"
        );
    }

    #[test]
    fn total_bonded_stake_with_multiple_entries() {
        let mut vs = RefinerSet::new();
        let id1 = test_id(b"refiner1");
        let id2 = test_id(b"refiner2");
        vs.bond(id1, MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id2, MIN_BOND_STAKE * 2, 0, 0).unwrap();
        assert_eq!(vs.total_bonded_stake(), MIN_BOND_STAKE * 3);
    }

    #[test]
    fn select_block_producer() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();
        let producer = vs.select_block_producer(0, 42);
        assert!(producer.is_some());
        assert_eq!(producer.unwrap().object_id, id);
    }

    #[test]
    fn per_entry_seniority_increases_weight() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
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
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up at timestamp 1,000,000 (~11.5 days)
        let top_up_time: u64 = 1_000_000;
        vs.bond(id.clone(), MIN_BOND_STAKE, 100, top_up_time)
            .unwrap();

        let v = vs.get_refiner(&id).unwrap();
        // Check age at ~1 year (31,557,600 seconds) — enough for measurable milli-years
        let check_time: u64 = 31_557_600;
        let age_0_milli = v.entries[0].age_years_milli(check_time);
        let age_1_milli = v.entries[1].age_years_milli(check_time);
        // Entry bonded at genesis should have ~31.7 milli-years at check_time
        assert!(
            age_0_milli > 0,
            "Entry bonded at genesis should have age after 1 year"
        );
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
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        // Entry 1: 1 OPL, Entry 2: 3 OPL
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 3, 100, 2000).unwrap();

        // Unbond 2 OPL: consumes entry 1 (1 OPL) + 1 OPL from entry 2
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE * 2, 500).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE * 2);

        let v = vs.get_refiner(&id).unwrap();
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].stake, MIN_BOND_STAKE * 2); // 3 - 1 = 2
        assert_eq!(v.entries[0].bonded_at_timestamp, 2000); // Keeps original timestamp
    }

    /// Full lifecycle integration test: bond → activate → unbond → mature → slash
    #[test]
    fn refiner_full_lifecycle() {
        let mut vs = RefinerSet::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        let charlie = test_id(b"charlie");

        // Phase 1: Three refiners bond at height 0
        vs.bond(alice.clone(), MIN_BOND_STAKE * 10, 0, 0).unwrap(); // Alice: 10 OPL
        vs.bond(bob.clone(), MIN_BOND_STAKE * 20, 0, 0).unwrap(); // Bob: 20 OPL
        vs.bond(charlie.clone(), MIN_BOND_STAKE * 5, 0, 0).unwrap(); // Charlie: 5 OPL

        // All start as Bonding
        assert_eq!(
            vs.get_refiner(&alice).unwrap().status,
            RefinerStatus::Bonding
        );
        assert_eq!(vs.get_refiner(&bob).unwrap().status, RefinerStatus::Bonding);
        assert_eq!(
            vs.get_refiner(&charlie).unwrap().status,
            RefinerStatus::Bonding
        );
        assert_eq!(vs.total_bonded_stake(), MIN_BOND_STAKE * 35);

        // Phase 2: Before epoch boundary, no activation
        let waiting = vs.activate_matured_refiners(EPOCH - 1);
        assert!(waiting.is_empty());

        // Phase 3: At epoch boundary, all refiners move to Waiting then Active
        let waiting = vs.activate_matured_refiners(EPOCH);
        assert_eq!(waiting.len(), 3);
        assert_eq!(
            vs.get_refiner(&alice).unwrap().status,
            RefinerStatus::Waiting
        );

        let (activated, _) = vs.rerank_refiners(0);
        assert_eq!(activated.len(), 3);
        assert_eq!(
            vs.get_refiner(&alice).unwrap().status,
            RefinerStatus::Active
        );
        assert_eq!(vs.get_refiner(&bob).unwrap().status, RefinerStatus::Active);
        assert_eq!(
            vs.get_refiner(&charlie).unwrap().status,
            RefinerStatus::Active
        );

        // Phase 4: Block producer selection — deterministic via seed
        let producer = vs.select_block_producer(0, 42).unwrap();
        // Bob has 2x the stake of Alice, so Bob should be selected more often
        // but Bob is not guaranteed — just verify selection works
        assert!(
            producer.object_id == alice
                || producer.object_id == bob
                || producer.object_id == charlie,
            "Producer must be one of the bonded refiners"
        );

        // Phase 5: Unbond Alice at height 2000, matures at height 2000 + EPOCH
        let unbonded = vs.unbond_amount(&alice, MIN_BOND_STAKE * 3, 2000).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE * 3);
        assert_eq!(vs.unbonding_queue.len(), 1);
        assert_eq!(vs.unbonding_queue[0].account, alice);
        assert_eq!(vs.unbonding_queue[0].amount, MIN_BOND_STAKE * 3);
        assert_eq!(vs.unbonding_queue[0].matures_at, 2000 + EPOCH);

        // Alice still has 7 OPL bonded
        assert_eq!(
            vs.get_refiner(&alice).unwrap().total_stake(),
            MIN_BOND_STAKE * 7
        );

        // Phase 6: Process matured unbonds — nothing before maturity
        let matured = vs.process_matured_unbonds(2000 + EPOCH - 1);
        assert!(matured.is_empty());

        // At maturity height, Alice's unbonding entry matures
        let matured = vs.process_matured_unbonds(2000 + EPOCH);
        assert_eq!(matured.len(), 1);
        assert_eq!(matured[0].0, alice);
        assert_eq!(matured[0].1, MIN_BOND_STAKE * 3);
        assert!(vs.unbonding_queue.is_empty());

        // Phase 7: Charlie double-signs — 100% slash, permanent Slashed.
        assert_eq!(
            vs.get_refiner(&charlie).unwrap().status,
            RefinerStatus::Active
        );

        // One strike: 100% burn of 5 OPL, permanent Slashed
        let burn = vs.slash_refiner(&charlie, 2100).unwrap();
        assert_eq!(burn, MIN_BOND_STAKE * 5); // all stake burned
        assert_eq!(
            vs.get_refiner(&charlie).unwrap().status,
            RefinerStatus::Slashed
        );
        assert_eq!(vs.get_refiner(&charlie).unwrap().total_stake(), 0);

        // Total burned = 5 OPL (original stake, 100% burn)
        assert_eq!(burn, MIN_BOND_STAKE * 5);

        // Slashed refiner excluded from Active set
        let active_count = vs.active_refiners().len();
        assert_eq!(active_count, 2); // Only Alice and Bob remain active

        // Total bonded stake excludes permanently slashed refiners
        let bonded = vs.total_bonded_stake();
        assert_eq!(bonded, MIN_BOND_STAKE * 7 + MIN_BOND_STAKE * 20); // Alice 7 + Bob 20
    }
}
