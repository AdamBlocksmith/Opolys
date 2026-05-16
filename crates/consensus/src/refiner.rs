//! # Proof-of-stake refiner management for Opolys.
//!
//! Opolys refiners bond stake as collateral and earn block rewards
//! proportional to their bonded stake:
//!
//! `weight = Σ entry.stake`
//!
//! Bond timestamps are retained for FIFO unbonding and provenance, but bond age
//! does not increase reward weight, finality weight, or producer selection odds.
//!
//! **Unbonding is FIFO** — when a refiner unbonds, the oldest entries are
//! consumed first. If the unbond amount exceeds an entry's stake, that entry
//! is fully consumed and the remainder comes from the next oldest. Split
//! entries keep their original `bonded_at_timestamp` for weight calculation.
//! The dynamic minimum only applies to new bond entries, not to residuals from
//! FIFO splits. It starts at 1 OPL and grows as `sqrt(total_issued_opl)`.
//!
//! **Slashing is narrowly scoped to double-signing only.** No governance
//! body can slash for other reasons. A slashed refiner's entire stake across
//! all entries is burned (not confiscated to any treasury), permanently
//! removing it from circulation.
//!
//! Block producers are selected by total-stake-weighted sampling among Active
//! refiners, where the seed is derived from on-chain entropy.

use borsh::{BorshDeserialize, BorshSerialize};
use opolys_core::{
    BLOCKS_PER_YEAR, EPOCH, FLAKES_PER_OPL, FlakeAmount, ObjectId, OpolysError, RefinerStatus,
};
use opolys_crypto::{Blake3Hasher, DOMAIN_STATE_ROOT};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

fn integer_sqrt_floor(n: u128) -> u128 {
    if n < 2 {
        return n;
    }

    let bit_len = 128 - n.leading_zeros() as u128;
    let mut left = 1u128;
    let mut right = 1u128 << bit_len.div_ceil(2);
    let mut result = 1u128;

    while left <= right {
        let mid = (left + right) / 2;
        let square = mid.saturating_mul(mid);
        if square <= n {
            result = mid;
            left = mid + 1;
        } else {
            right = mid - 1;
        }
    }

    result
}

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

fn mix_seed(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn uniform_amount_from_seed(seed: u64, upper: FlakeAmount) -> Option<FlakeAmount> {
    if upper == 0 {
        return None;
    }

    let upper = upper as u128;
    let range = 1u128 << 64;
    let unbiased_zone = range - (range % upper);
    let mut candidate = seed;

    loop {
        let value = candidate as u128;
        if value < unbiased_zone {
            return Some((value % upper) as FlakeAmount);
        }
        candidate = mix_seed(candidate);
    }
}

/// A single bond entry within a refiner's stake.
///
/// Each entry has its own stake amount and bonding timestamp.
/// Entries are consumed in FIFO order during unbonding — oldest first.
/// Split entries retain their original `bonded_at_timestamp` for provenance
/// and FIFO ordering.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BondEntry {
    /// Amount of OPL (in flakes) locked in this entry.
    pub stake: FlakeAmount,
    /// Block height at which this entry was bonded.
    pub bonded_at_height: u64,
    /// Unix timestamp at which this entry was bonded. Used for FIFO/provenance,
    /// not for reward or finality weighting.
    pub bonded_at_timestamp: u64,
}

impl BondEntry {
    /// Compute this entry's age in milli-years (× 1000) based on the time
    /// elapsed since bonding. Returns 0 if `current_timestamp` is at or before
    /// the bonding timestamp.
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

    /// Compute this entry's weight.
    ///
    /// Weight is exactly stake. Bond age is intentionally ignored so refiner
    /// economics do not accrue value merely from time passing.
    pub fn weight(&self, current_timestamp: u64) -> FlakeAmount {
        crate::emission::compute_refiner_weight(self.stake, self.age_years_milli(current_timestamp))
    }
}

/// Information about a bonded refiner.
///
/// Refiners hold one or more bond entries. Total weight is the sum of bonded
/// stake across entries.
///
/// Double-signing triggers graduated slashing: 10% burn on first offense,
/// 33% burn + suspension on second, 100% burn + permanent Slashed on third+.
/// Slashed stake is removed from circulation, not transferred to any treasury.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct RefinerInfo {
    /// The refiner's on-chain identity (Blake3 hash of public key).
    pub object_id: ObjectId,
    /// The refiner's bond entries, each with its own stake and timestamp.
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

    /// Compute the refiner's total weight as the sum of per-entry stake.
    pub fn weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.entries
            .iter()
            .map(|e| e.weight(current_timestamp))
            .sum()
    }

    /// Add a new bond entry (top-up) to this refiner.
    ///
    /// Each new entry must meet the dynamic minimum bond.
    /// The entry gets its own timestamp for FIFO/provenance.
    /// If an entry with the same `bonded_at_timestamp` already exists,
    /// stakes are merged (auto-merge) to reduce entry count.
    fn add_entry(&mut self, stake: FlakeAmount, height: u64, timestamp: u64) -> Result<(), String> {
        // Auto-merge: if an entry with the same timestamp exists, combine stakes
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.bonded_at_timestamp == timestamp)
        {
            existing.stake = existing.stake.checked_add(stake).ok_or_else(|| {
                format!(
                    "Bond entry stake overflow: existing {} + new {}",
                    existing.stake, stake
                )
            })?;
            return Ok(());
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
        Ok(())
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

/// The set of all bonded refiners, supporting bonding, unbonding,
/// activating, slashing, and stake-weighted block-producer selection.
///
/// Only **double-signing** triggers slashing in Opolys — no other offense
/// results in stake removal. Slashed stake is burned (removed from supply),
/// not sent to any entity.
///
/// Supports a dynamic active set derived from issued supply:
/// `active_limit = EPOCH + sqrt(total_issued_opl)`.
/// Refiners outside the active limit by total stake sit in Waiting status.
/// rerank_refiners() at epoch boundaries promotes/demotes as stake changes.
#[derive(Debug)]
pub struct RefinerSet {
    /// In-memory refiner set. All refiners stay resident because totals,
    /// producer selection, and state-root computation are consensus-critical.
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

    /// Consensus state must not evict refiners without a deterministic backing
    /// store. This is intentionally a no-op until such a store exists.
    fn evict_cache_if_full(&mut self) {}

    /// Clear the dirty set (called after state root is committed).
    pub fn clear_dirty(&mut self) {
        self.dirty_refiners.clear();
    }

    /// Compute the minimum new bond entry from issued supply.
    ///
    /// `minimum_bond = max(1 OPL, sqrt(total_issued_opl))`.
    ///
    /// Early chains remain accessible at 1 OPL. As the economy grows, refiner
    /// status requires more serious bonded capital without adding a new
    /// protocol constant.
    pub fn minimum_bond_stake(total_issued_flakes: FlakeAmount) -> FlakeAmount {
        let total_issued_opl = total_issued_flakes / FLAKES_PER_OPL;
        let min_opl = integer_sqrt_floor(total_issued_opl as u128).max(1);
        min_opl
            .saturating_mul(FLAKES_PER_OPL as u128)
            .min(FlakeAmount::MAX as u128) as FlakeAmount
    }

    /// Bond stake as a refiner entry. If the refiner doesn't exist, creates
    /// a new refiner with this as their first entry (status: Bonding). If the
    /// refiner already exists, adds to the existing entry (auto-merge) if same
    /// timestamp, or creates a new entry (top-up) with its own timestamp.
    ///
    /// Each **new** entry must meet `minimum_bond_stake(total_issued_flakes)`.
    /// Merged entries have no minimum since they may be residuals from FIFO
    /// splits.
    pub fn bond(
        &mut self,
        object_id: ObjectId,
        stake: FlakeAmount,
        height: u64,
        timestamp: u64,
        total_issued_flakes: FlakeAmount,
    ) -> Result<(), String> {
        let minimum_bond = Self::minimum_bond_stake(total_issued_flakes);
        if stake < minimum_bond {
            return Err(format!(
                "Insufficient stake per entry: need {}, have {}",
                minimum_bond, stake
            ));
        }

        if let Some(refiner) = self.cached_refiners.get_mut(&object_id) {
            if refiner.status == RefinerStatus::Slashed {
                return Err("Slashed refiners cannot re-bond".to_string());
            }
            refiner.add_entry(stake, height, timestamp)?;
            if refiner.status == RefinerStatus::Unbonding {
                refiner.status = RefinerStatus::Bonding;
            }
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
    /// Requests must be non-zero and cannot exceed the refiner's current total
    /// stake. If the refiner has no remaining entries after unbonding, they
    /// move to `Unbonding` until the queued withdrawal matures. Pending
    /// unbonding stake remains slashable during that delay.
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
        if amount == 0 {
            return Err("Unbond amount must be greater than zero".to_string());
        }
        if amount > total_stake {
            return Err(format!(
                "Cannot unbond more than total stake: requested {}, available {}",
                amount, total_stake
            ));
        }

        let unbonded = refiner.unbond_fifo(amount);

        // Enqueue the unbonding entry with a maturation height
        let matures_at = current_height.saturating_add(EPOCH);
        self.unbonding_queue.push(PendingUnbond {
            account: object_id.clone(),
            amount: unbonded,
            matures_at,
        });

        // If no entries remain, keep a slashable Unbonding marker until the
        // queued withdrawal matures.
        if refiner.entries.is_empty() {
            refiner.status = RefinerStatus::Unbonding;
            self.active_set.retain(|id| id != object_id);
        }
        self.dirty_refiners.insert(object_id.clone());

        Ok(unbonded)
    }

    /// Return all matured unbonding entries at the given block height without
    /// removing them from the queue.
    pub fn matured_unbonds(&self, current_height: u64) -> Vec<(ObjectId, FlakeAmount)> {
        self.unbonding_queue
            .iter()
            .filter(|entry| current_height >= entry.matures_at)
            .map(|entry| (entry.account.clone(), entry.amount))
            .collect()
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
        for (account, _) in &matured {
            let still_pending = self
                .unbonding_queue
                .iter()
                .any(|entry| &entry.account == account);
            let should_remove = self.cached_refiners.get(account).is_some_and(|refiner| {
                refiner.status == RefinerStatus::Unbonding
                    && refiner.total_stake() == 0
                    && !still_pending
            });
            if should_remove {
                self.cached_refiners.remove(account);
                self.active_set.retain(|id| id != account);
            }
        }
        matured
    }

    /// Activate all refiners that have been bonding for at least one full epoch.
    ///
    /// Moves Bonding → Waiting. rerank_refiners() handles the Waiting → Active
    /// promotion (top-N by stake). This separation allows fair competition for
    /// active slots: all epoch-matured refiners become eligible, then the highest-
    /// stake ones are promoted.
    pub fn activate_matured_refiners(&mut self, current_height: u64) -> Vec<ObjectId> {
        let mut newly_waiting = Vec::new();

        // Collect eligible IDs first to avoid borrow conflict
        let eligible: Vec<ObjectId> = self
            .cached_refiners
            .iter()
            .filter(|(_, v)| {
                v.status == RefinerStatus::Bonding
                    && v.entries
                        .first()
                        .is_some_and(|e| current_height >= e.bonded_at_height.saturating_add(EPOCH))
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

    /// Compute the active-refiner limit from issued supply.
    ///
    /// `active_limit = EPOCH + sqrt(total_issued_opl)`, where
    /// `total_issued_opl = total_issued_flakes / FLAKES_PER_OPL`.
    ///
    /// This keeps refiner processing bounded at launch while letting the active
    /// set grow organically as the OPL economy grows.
    pub fn active_refiner_limit(total_issued_flakes: FlakeAmount) -> usize {
        let total_issued_opl = total_issued_flakes / FLAKES_PER_OPL;
        let sqrt_issued = integer_sqrt_floor(total_issued_opl as u128);
        let limit = (EPOCH as u128).saturating_add(sqrt_issued);
        limit.min(usize::MAX as u128) as usize
    }

    /// Re-rank all eligible refiners at an epoch boundary.
    ///
    /// Collects all non-Slashed refiners with stake > 0, sorts by total stake
    /// descending, promotes the top active_refiner_limit(total_issued) to Active,
    /// and demotes the rest to Waiting. Returns (newly_activated, newly_demoted)
    /// for logging.
    ///
    /// Must be called at epoch boundaries (height % EPOCH == 0) after
    /// activate_matured_refiners().
    pub fn rerank_refiners(
        &mut self,
        _current_timestamp: u64,
        total_issued_flakes: FlakeAmount,
    ) -> (Vec<ObjectId>, Vec<ObjectId>) {
        let active_limit = Self::active_refiner_limit(total_issued_flakes);

        // Sort all eligible refiners by stake descending.
        let mut eligible: Vec<(ObjectId, u64)> = self
            .cached_refiners
            .iter()
            .filter(|(_, v)| v.status != RefinerStatus::Slashed && v.total_stake() > 0)
            .map(|(id, v)| (id.clone(), v.total_stake()))
            .collect();
        eligible.sort_by(|(a_id, a_weight), (b_id, b_weight)| {
            b_weight.cmp(a_weight).then_with(|| a_id.0.0.cmp(&b_id.0.0))
        });

        let mut newly_activated = Vec::new();
        let mut newly_demoted = Vec::new();

        for (i, (id, _)) in eligible.iter().enumerate() {
            if let Some(v) = self.cached_refiners.get_mut(id) {
                if i < active_limit {
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

    /// Apply surplus-based stake decay to bonded refiners.
    ///
    /// The baseline security stake is derived from the current issued supply:
    /// `minimum_bond_stake(total_issued) * active_refiner_limit(total_issued)`.
    /// If bonded stake is at or below that baseline, no stake decays. If bonded
    /// stake exceeds the baseline, only the surplus pressure creates a small
    /// epoch burn. The annualized surplus pressure is damped by the square root
    /// of the active refiner capacity, then prorated across one epoch:
    /// `entry_decay = entry_stake * surplus_stake * EPOCH / (total_bonded_stake * BLOCKS_PER_YEAR * sqrt(active_refiner_limit))`.
    ///
    /// Returns the total amount of stake burned across all refiners.
    pub fn decay_stake(&mut self, total_issued_flakes: FlakeAmount) -> FlakeAmount {
        let total_bonded = self.total_bonded_stake();
        if total_bonded == 0 {
            return 0;
        }

        let active_refiner_limit = Self::active_refiner_limit(total_issued_flakes);
        let baseline_security_stake =
            Self::minimum_bond_stake(total_issued_flakes) as u128 * active_refiner_limit as u128;
        let surplus_stake = (total_bonded as u128).saturating_sub(baseline_security_stake);
        if surplus_stake == 0 {
            return 0;
        }
        let active_limit_sqrt = integer_sqrt_floor(active_refiner_limit as u128).max(1);

        let mut total_burned: FlakeAmount = 0;
        let mut dirty = Vec::new();
        for (id, refiner) in &mut self.cached_refiners {
            if refiner.status == RefinerStatus::Slashed {
                continue;
            }
            let mut refiner_burned = 0u64;
            for entry in &mut refiner.entries {
                if entry.stake == 0 {
                    continue;
                }
                let burned = (entry.stake as u128)
                    .saturating_mul(surplus_stake)
                    .saturating_mul(EPOCH as u128)
                    .checked_div(total_bonded as u128)
                    .unwrap_or(0)
                    .checked_div(BLOCKS_PER_YEAR as u128)
                    .unwrap_or(0)
                    .checked_div(active_limit_sqrt)
                    .unwrap_or(0)
                    .min(entry.stake as u128) as FlakeAmount;
                if burned == 0 {
                    continue;
                }
                total_burned = total_burned.saturating_add(burned);
                refiner_burned = refiner_burned.saturating_add(burned);
                entry.stake = entry.stake.saturating_sub(burned);
            }
            if refiner_burned > 0 {
                dirty.push(id.clone());
            }
        }
        for id in dirty {
            self.dirty_refiners.insert(id);
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
    /// Returns the Flake amount burned. Pending unbonding entries are burned too,
    /// because stake remains in protocol custody until maturity. Returns `Ok(0)`
    /// if the refiner is already `Slashed` and has no pending unbonding entries.
    pub fn slash_refiner(
        &mut self,
        object_id: &ObjectId,
        _current_height: u64,
    ) -> Result<FlakeAmount, String> {
        let mut pending_burn = 0u64;
        let mut remaining_unbonds = Vec::with_capacity(self.unbonding_queue.len());
        for entry in self.unbonding_queue.drain(..) {
            if &entry.account == object_id {
                pending_burn = pending_burn.saturating_add(entry.amount);
            } else {
                remaining_unbonds.push(entry);
            }
        }
        self.unbonding_queue = remaining_unbonds;

        let refiner = match self.cached_refiners.get_mut(object_id) {
            Some(refiner) => refiner,
            None if pending_burn > 0 => return Ok(pending_burn),
            None => return Err("Refiner not found".to_string()),
        };

        // Already permanently slashed — nothing more to take
        if refiner.status == RefinerStatus::Slashed {
            return Ok(pending_burn);
        }

        // 100% burn; permanent Slashed status
        let burn = refiner.total_stake().saturating_add(pending_burn);
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

    /// Total stake across all Bonding, Waiting, and Active refiners. Used for
    /// stake coverage, difficulty-floor pressure, and security observability.
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
        let mut refiners: Vec<RefinerInfo> = self.cached_refiners.values().cloned().collect();
        refiners.sort_by_key(|v| v.object_id.0.0);
        refiners
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
    pub fn compute_state_root(&self) -> Result<opolys_core::Hash, OpolysError> {
        let mut sorted_ids: Vec<&ObjectId> = self.cached_refiners.keys().collect();
        sorted_ids.sort_by_key(|a| a.0.0);

        let mut hasher = Blake3Hasher::new();
        hasher.update(DOMAIN_STATE_ROOT);
        hasher.update(b"refiners");

        // Hash all refiner state (sorted by ObjectId)
        for id in sorted_ids {
            if let Some(refiner) = self.cached_refiners.get(id) {
                let bytes = borsh::to_vec(refiner).map_err(|e| {
                    OpolysError::SerializationError(format!("Refiner serialization failed: {}", e))
                })?;
                hasher.update(&bytes);
            }
        }

        // Hash the unbonding queue (order matters — it's FIFO)
        for entry in &self.unbonding_queue {
            let bytes = borsh::to_vec(entry).map_err(|e| {
                OpolysError::SerializationError(format!(
                    "Unbonding entry serialization failed: {}",
                    e
                ))
            })?;
            hasher.update(&bytes);
        }

        Ok(hasher.finalize())
    }

    /// Select the next block producer via total-stake-weighted sampling.
    ///
    /// This keeps production split-neutral: splitting one stake across many
    /// accounts does not create more aggregate producer weight.
    /// The `seed` parameter provides on-chain entropy to make the selection
    /// deterministic and verifiable. Returns `None` if there are no active
    /// refiners.
    pub fn select_block_producer(&self, seed: u64) -> Option<&RefinerInfo> {
        let mut active: Vec<&RefinerInfo> = self
            .active_set
            .iter()
            .filter_map(|id| self.cached_refiners.get(id))
            .filter(|v| v.total_stake() > 0)
            .collect();
        active.sort_by_key(|v| v.object_id.0.0);

        let total_active_stake = active.iter().fold(0u64, |acc, refiner| {
            acc.saturating_add(refiner.total_stake())
        });
        let ticket = uniform_amount_from_seed(seed, total_active_stake)?;
        let mut cumulative = 0u64;
        for refiner in active {
            cumulative = cumulative.saturating_add(refiner.total_stake());
            if ticket < cumulative {
                return Some(refiner);
            }
        }
        None
    }
}

impl Default for RefinerSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::MIN_BOND_STAKE;
    use opolys_crypto::hash_to_object_id;

    fn test_id(seed: &[u8]) -> ObjectId {
        hash_to_object_id(seed)
    }

    #[test]
    fn bond_new_refiner() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        assert_eq!(vs.refiner_count(), 1);
        assert_eq!(vs.get_refiner(&id).unwrap().total_stake(), MIN_BOND_STAKE);
        assert_eq!(vs.get_refiner(&id).unwrap().entries.len(), 1);
    }

    #[test]
    fn bond_insufficient_stake_per_entry() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        // MIN_BOND_STAKE is now 1 OPL = 1,000,000 flakes
        assert!(vs.bond(id, 100, 0, 0, 0).is_err());
    }

    #[test]
    fn active_refiner_limit_grows_with_issued_supply() {
        assert_eq!(RefinerSet::active_refiner_limit(0), EPOCH as usize);
        assert_eq!(
            RefinerSet::active_refiner_limit(1_000_000 * FLAKES_PER_OPL),
            EPOCH as usize + 1_000
        );
        assert_eq!(
            RefinerSet::active_refiner_limit(25_000_000 * FLAKES_PER_OPL),
            EPOCH as usize + 5_000
        );
    }

    #[test]
    fn minimum_bond_stake_grows_with_issued_supply() {
        assert_eq!(RefinerSet::minimum_bond_stake(0), MIN_BOND_STAKE);
        assert_eq!(
            RefinerSet::minimum_bond_stake(1_000_000 * FLAKES_PER_OPL),
            1_000 * FLAKES_PER_OPL
        );
        assert_eq!(
            RefinerSet::minimum_bond_stake(25_000_000 * FLAKES_PER_OPL),
            5_000 * FLAKES_PER_OPL
        );
    }

    #[test]
    fn surplus_decay_is_zero_below_security_baseline() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        let burned = vs.decay_stake(0);
        assert_eq!(burned, 0);
        assert_eq!(
            vs.get_refiner(&id).unwrap().total_stake(),
            MIN_BOND_STAKE * 10
        );
    }

    #[test]
    fn surplus_decay_burns_only_when_bonded_stake_exceeds_baseline() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 1000, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        let baseline =
            RefinerSet::minimum_bond_stake(0) as u128 * RefinerSet::active_refiner_limit(0) as u128;
        let total_bonded = MIN_BOND_STAKE * 1000;
        let surplus = total_bonded as u128 - baseline;
        let active_limit_sqrt = integer_sqrt_floor(RefinerSet::active_refiner_limit(0) as u128);
        let expected = ((total_bonded as u128 * surplus / total_bonded as u128) * EPOCH as u128
            / BLOCKS_PER_YEAR as u128
            / active_limit_sqrt) as FlakeAmount;

        let burned = vs.decay_stake(0);
        assert_eq!(burned, expected);
        assert!(burned > 0);
        assert_eq!(
            vs.get_refiner(&id).unwrap().total_stake(),
            total_bonded - expected
        );
        assert!(vs.dirty_refiners.contains(&id));
    }

    #[test]
    fn rerank_refiners_ties_break_by_object_id() {
        let mut vs = RefinerSet::new();
        let ids = [
            test_id(b"tie-refiner-c"),
            test_id(b"tie-refiner-a"),
            test_id(b"tie-refiner-b"),
        ];

        for id in &ids {
            vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        }
        vs.activate_matured_refiners(EPOCH);
        vs.rerank_refiners(0, 0);

        let mut expected = ids.to_vec();
        expected.sort_by_key(|id| id.0.0);
        assert_eq!(vs.active_set_ids(), &expected);
    }

    #[test]
    fn top_up_bond_adds_entry() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up: add a second entry at a different timestamp
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000, 0)
            .unwrap();

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
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 100, 0).unwrap();

        // Top-up at same timestamp — should auto-merge
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 5, 100, 0).unwrap();

        let v = vs.get_refiner(&id).unwrap();
        assert_eq!(v.entries.len(), 1); // Merged, not two entries
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);
    }

    #[test]
    fn unbond_fifo_consumes_oldest_first() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap(); // Entry 1: 1 OPL at t=0
        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 100, 1000, 0)
            .unwrap(); // Entry 2: 2 OPL at t=1000

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
        // Split entry keeps original timestamp for FIFO/provenance.
        assert_eq!(v.entries[0].bonded_at_timestamp, 1000);

        // The unbonded amount should be in the unbonding queue
        assert_eq!(vs.unbonding_queue.len(), 1);
        assert_eq!(
            vs.unbonding_queue[0].amount,
            MIN_BOND_STAKE + MIN_BOND_STAKE / 2
        );
        assert_eq!(vs.unbonding_queue[0].matures_at, 500 + EPOCH);
    }

    #[test]
    fn unbond_fifo_marks_refiner_unbonding_when_empty() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();

        // Unbond the entire stake
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);
        assert_eq!(vs.refiner_count(), 1);
        assert_eq!(
            vs.get_refiner(&id).unwrap().status,
            RefinerStatus::Unbonding
        );
        assert_eq!(vs.get_refiner(&id).unwrap().total_stake(), 0);
        // Unbonding queue holds the pending entry
        assert_eq!(vs.unbonding_queue.len(), 1);
    }

    #[test]
    fn unbond_more_than_stake() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();

        // Try to unbond more than total stake
        let result = vs.unbond_amount(&id, MIN_BOND_STAKE * 10, 100);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Cannot unbond more than total stake")
        );
        assert_eq!(vs.refiner_count(), 1);
        assert!(vs.unbonding_queue.is_empty());
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
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();

        // Unbond at height 100, matures at 100 + EPOCH
        let unbonded = vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(unbonded, MIN_BOND_STAKE);
        assert_eq!(
            vs.get_refiner(&id).unwrap().status,
            RefinerStatus::Unbonding
        );

        // One block before maturity — nothing matured yet
        let matured = vs.process_matured_unbonds(100 + EPOCH - 1);
        assert!(matured.is_empty());
        assert_eq!(vs.unbonding_queue.len(), 1);
        assert!(vs.get_refiner(&id).is_some());

        // At maturity height, the entry matures
        let matured = vs.process_matured_unbonds(100 + EPOCH);
        assert_eq!(matured.len(), 1);
        assert_eq!(matured[0].0, id);
        assert_eq!(matured[0].1, MIN_BOND_STAKE);
        assert!(vs.unbonding_queue.is_empty());
        assert!(vs.get_refiner(&id).is_none());
    }

    #[test]
    fn matured_unbonds_preview_does_not_drain_queue() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();

        let matured = vs.matured_unbonds(100 + EPOCH);
        assert_eq!(matured, vec![(id, MIN_BOND_STAKE)]);
        assert_eq!(vs.unbonding_queue.len(), 1);
    }

    #[test]
    fn activate_matured_refiners_transitions_after_epoch() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();

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
        let (activated, demoted) = vs.rerank_refiners(0, 0);
        assert_eq!(activated.len(), 1);
        assert!(demoted.is_empty());
        assert_eq!(vs.get_refiner(&id).unwrap().status, RefinerStatus::Active);
    }

    #[test]
    fn dynamic_refiner_limit_holds_excess_in_waiting() {
        // Bond active_refiner_limit(0) + 2 refiners. After activate_matured_refiners
        // all move to Waiting. After rerank_refiners, exactly active_refiner_limit(0)
        // become Active and 2 remain Waiting.

        let mut vs = RefinerSet::new();

        let active_limit = RefinerSet::active_refiner_limit(0);
        let n = active_limit + 2;
        let mut ids = Vec::new();
        for i in 0..n {
            let seed = format!("refiner_{}", i);
            let id = test_id(seed.as_bytes());
            vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
            ids.push(id);
        }
        assert_eq!(vs.total_active_refiners(), 0);
        assert_eq!(vs.total_bonding_refiners(), n);

        // All move to Waiting at epoch boundary
        let waiting = vs.activate_matured_refiners(EPOCH);
        assert_eq!(waiting.len(), n);
        assert_eq!(vs.total_active_refiners(), 0);
        assert_eq!(vs.total_waiting_refiners(), n);

        // rerank promotes top active_limit refiners to Active
        let (activated, demoted) = vs.rerank_refiners(0, 0);
        assert_eq!(activated.len(), active_limit);
        assert!(demoted.is_empty());
        assert_eq!(vs.total_active_refiners(), active_limit);
        assert_eq!(vs.total_waiting_refiners(), 2);

        // A second rerank at the same height — no change (cap still full)
        let (activated2, demoted2) = vs.rerank_refiners(0, 0);
        assert!(activated2.is_empty());
        assert!(demoted2.is_empty());
        assert_eq!(vs.total_active_refiners(), active_limit);
    }

    #[test]
    fn rerank_refiners_uses_stake_weight() {
        let mut vs = RefinerSet::new();
        let active_limit = RefinerSet::active_refiner_limit(0);
        let old_small = test_id(b"old-small-refiner");
        let fresh_large = test_id(b"fresh-large-refiner");

        vs.bond(old_small.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.bond(fresh_large.clone(), MIN_BOND_STAKE * 2, 0, 31_557_600, 0)
            .unwrap();

        for i in 0..(active_limit - 1) {
            let id = test_id(format!("large-filler-{i}").as_bytes());
            vs.bond(id, MIN_BOND_STAKE * 2, 0, 31_557_600, 0).unwrap();
        }

        vs.activate_matured_refiners(EPOCH);
        vs.rerank_refiners(31_557_600, 0);

        assert_eq!(
            vs.get_refiner(&old_small).unwrap().status,
            RefinerStatus::Waiting
        );
        assert_eq!(
            vs.get_refiner(&fresh_large).unwrap().status,
            RefinerStatus::Active
        );
    }

    #[test]
    fn slash_refiner_burns_all_stake() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0, 0).unwrap(); // 10 OPL
        vs.activate(&id, 1).unwrap();

        let burned = vs.slash_refiner(&id, 100).unwrap();
        assert_eq!(burned, MIN_BOND_STAKE * 10); // 100% burned
        let v = vs.get_refiner(&id).unwrap();
        assert_eq!(v.total_stake(), 0);
        assert_eq!(v.status, RefinerStatus::Slashed);
    }

    #[test]
    fn slash_refiner_burns_pending_unbonding_stake() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE * 10, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.unbond_amount(&id, MIN_BOND_STAKE * 4, 100).unwrap();
        assert_eq!(
            vs.get_refiner(&id).unwrap().total_stake(),
            MIN_BOND_STAKE * 6
        );
        assert_eq!(vs.unbonding_queue.len(), 1);

        let burned = vs.slash_refiner(&id, 200).unwrap();
        assert_eq!(burned, MIN_BOND_STAKE * 10);
        assert!(vs.unbonding_queue.is_empty());
        let v = vs.get_refiner(&id).unwrap();
        assert_eq!(v.total_stake(), 0);
        assert_eq!(v.status, RefinerStatus::Slashed);
    }

    #[test]
    fn rebond_during_unbonding_returns_to_bonding() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.unbond_amount(&id, MIN_BOND_STAKE, 100).unwrap();
        assert_eq!(
            vs.get_refiner(&id).unwrap().status,
            RefinerStatus::Unbonding
        );
        assert_eq!(vs.unbonding_queue.len(), 1);

        vs.bond(id.clone(), MIN_BOND_STAKE * 2, 200, 200, 0)
            .unwrap();
        let refiner = vs.get_refiner(&id).unwrap();
        assert_eq!(refiner.status, RefinerStatus::Bonding);
        assert_eq!(refiner.total_stake(), MIN_BOND_STAKE * 2);
        assert_eq!(vs.unbonding_queue.len(), 1);

        let burned = vs.slash_refiner(&id, 300).unwrap();
        assert_eq!(burned, MIN_BOND_STAKE * 3);
        assert!(vs.unbonding_queue.is_empty());
    }

    #[test]
    fn slash_refiner_already_slashed_returns_zero() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.slash_refiner(&id, 100).unwrap(); // permanently slashed

        let burned = vs.slash_refiner(&id, 200).unwrap(); // idempotent
        assert_eq!(burned, 0);
    }

    #[test]
    fn record_correct_attestation_increments_active_refiner() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"attester");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
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
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        vs.slash_refiner(&id, 100).unwrap(); // slashed

        let result = vs.bond(id.clone(), MIN_BOND_STAKE, 400, 400, 0);
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
        vs.bond(id1, MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.bond(id2, MIN_BOND_STAKE * 2, 0, 0, 0).unwrap();
        assert_eq!(vs.total_bonded_stake(), MIN_BOND_STAKE * 3);
    }

    #[test]
    fn select_block_producer() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();
        let producer = vs.select_block_producer(42);
        assert!(producer.is_some());
        assert_eq!(producer.unwrap().object_id, id);
    }

    #[test]
    fn select_block_producer_is_total_stake_weighted() {
        let mut vs = RefinerSet::new();
        let low_stake = test_id(b"low-stake-refiner");
        let high_stake = test_id(b"high-stake-refiner");
        vs.bond(low_stake.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.bond(high_stake.clone(), MIN_BOND_STAKE * 100, 0, 0, 0)
            .unwrap();
        vs.activate(&low_stake, 1).unwrap();
        vs.activate(&high_stake, 1).unwrap();

        let mut low_count = 0;
        let mut high_count = 0;
        for seed in 0..10_000 {
            let producer = vs.select_block_producer(mix_seed(seed)).unwrap();
            if producer.object_id == low_stake {
                low_count += 1;
            } else if producer.object_id == high_stake {
                high_count += 1;
            }
        }

        assert!(
            high_count > low_count * 20,
            "100x stake should dominate producer selection; low={low_count}, high={high_count}"
        );
    }

    #[test]
    fn per_entry_age_does_not_increase_weight() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 100, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        let weight_at_bond = vs.total_weight(100);
        let one_year_secs = (365.25 * 24.0 * 3600.0) as u64;
        let weight_after_year = vs.total_weight(100 + one_year_secs);

        assert_eq!(weight_after_year, weight_at_bond);
    }

    #[test]
    fn top_up_entry_age_is_metadata_only() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.activate(&id, 1).unwrap();

        // Top-up at timestamp 1,000,000 (~11.5 days)
        let top_up_time: u64 = 1_000_000;
        vs.bond(id.clone(), MIN_BOND_STAKE, 100, top_up_time, 0)
            .unwrap();

        let v = vs.get_refiner(&id).unwrap();
        // Check age at ~1 year (31,557,600 seconds) — retained for provenance/FIFO.
        let check_time: u64 = 31_557_600;
        let age_0_milli = v.entries[0].age_years_milli(check_time);
        let age_1_milli = v.entries[1].age_years_milli(check_time);
        assert!(
            age_0_milli > 0,
            "Entry bonded at genesis should have age after 1 year"
        );
        assert!(age_1_milli > 0, "Top-up entry should have age after 1 year");
        assert_eq!(v.entries[1].age_years_milli(top_up_time), 0);
        assert_eq!(v.weight(check_time), v.total_stake());
    }

    #[test]
    fn stake_coverage() {
        let coverage = crate::emission::compute_stake_coverage(500_000, 1_000_000);
        assert_eq!(coverage, 500);
    }

    #[test]
    fn unbond_fifo_partial_from_second_entry() {
        let mut vs = RefinerSet::new();
        let id = test_id(b"refiner1");
        // Entry 1: 1 OPL, Entry 2: 3 OPL
        vs.bond(id.clone(), MIN_BOND_STAKE, 0, 0, 0).unwrap();
        vs.bond(id.clone(), MIN_BOND_STAKE * 3, 100, 2000, 0)
            .unwrap();

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
        vs.bond(alice.clone(), MIN_BOND_STAKE * 10, 0, 0, 0)
            .unwrap(); // Alice: 10 OPL
        vs.bond(bob.clone(), MIN_BOND_STAKE * 20, 0, 0, 0).unwrap(); // Bob: 20 OPL
        vs.bond(charlie.clone(), MIN_BOND_STAKE * 5, 0, 0, 0)
            .unwrap(); // Charlie: 5 OPL

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

        let (activated, _) = vs.rerank_refiners(0, 0);
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
        let producer = vs.select_block_producer(42).unwrap();
        // Producer selection is stake-weighted among active refiners.
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
