use opolys_core::{FlakeAmount, ObjectId, ValidatorStatus, MIN_BOND_STAKE};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ValidatorInfo {
    pub object_id: ObjectId,
    pub stake: FlakeAmount,
    pub bonded_at_height: u64,
    pub bonded_at_timestamp: u64,
    pub status: ValidatorStatus,
    pub last_signed_height: u64,
}

impl ValidatorInfo {
    pub fn age_years(&self, current_timestamp: u64) -> f64 {
        if current_timestamp <= self.bonded_at_timestamp {
            return 0.0;
        }
        let age_secs = current_timestamp - self.bonded_at_timestamp;
        age_secs as f64 / (365.25 * 24.0 * 3600.0)
    }
}

#[derive(Debug)]
pub struct ValidatorSet {
    validators: HashMap<ObjectId, ValidatorInfo>,
}

impl ValidatorSet {
    pub fn new() -> Self {
        ValidatorSet {
            validators: HashMap::new(),
        }
    }

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

    pub fn unbond(&mut self, object_id: &ObjectId) -> Result<ValidatorInfo, String> {
        let info = self.validators.remove(object_id)
            .ok_or_else(|| "Validator not bonded".to_string())?;
        Ok(info)
    }

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

    pub fn slash(&mut self, object_id: &ObjectId) -> Result<FlakeAmount, String> {
        let validator = self.validators.get_mut(object_id)
            .ok_or_else(|| "Validator not found".to_string())?;
        let slashed_amount = validator.stake;
        validator.status = ValidatorStatus::Slashed;
        validator.stake = 0;
        Ok(slashed_amount)
    }

    pub fn get_validator(&self, object_id: &ObjectId) -> Option<&ValidatorInfo> {
        self.validators.get(object_id)
    }

    pub fn total_bonded_stake(&self) -> FlakeAmount {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active || v.status == ValidatorStatus::Bonding)
            .map(|v| v.stake)
            .sum()
    }

    pub fn active_validators(&self) -> Vec<&ValidatorInfo> {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .collect()
    }

    pub fn total_weight(&self, current_timestamp: u64) -> FlakeAmount {
        self.validators.values()
            .filter(|v| v.status == ValidatorStatus::Active)
            .map(|v| crate::emission::compute_validator_weight(v.stake, v.age_years(current_timestamp)))
            .sum()
    }

    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

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

        let mut cumulative = 0u64;
        let target = seed % total_weight;
        for v in &active {
            let weight = crate::emission::compute_validator_weight(v.stake, v.age_years(current_timestamp));
            cumulative += weight;
            if cumulative > target {
                return Some(*v);
            }
        }

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