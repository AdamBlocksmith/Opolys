use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub type FlakeAmount = u64;
pub type BlockHeight = u64;
pub type PublicKey = Vec<u8>;
pub type Signature = Vec<u8>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct Hash(pub [u8; 32]);

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as Deserialize>::deserialize(deserializer)?;
        let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("Hash must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Hash(arr))
    }
}

impl Hash {
    pub fn zero() -> Self {
        Hash([0u8; 32])
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ObjectId(pub Hash);

impl ObjectId {
    pub fn zero() -> Self {
        ObjectId(Hash::zero())
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionAction {
    Transfer { recipient: ObjectId, amount: FlakeAmount },
    ValidatorBond,
    ValidatorUnbond,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusPhase {
    ProofOfWork,
    ProofOfStake,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidatorStatus {
    None,
    Bonding,
    Active,
    Unbonding,
    Slashed,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BlockHeader {
    pub height: BlockHeight,
    pub previous_hash: Hash,
    pub state_root: Hash,
    pub transaction_root: Hash,
    pub timestamp: u64,
    pub difficulty: u64,
    pub pow_proof: Option<Vec<u8>>,
    pub validator_signature: Option<Vec<u8>>,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Transaction {
    pub tx_id: ObjectId,
    pub sender: ObjectId,
    pub action: TransactionAction,
    pub fee: FlakeAmount,
    pub signature: Vec<u8>,
    pub nonce: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
}

pub fn opl_to_flake(opl: u64) -> FlakeAmount {
    opl.saturating_mul(crate::FLAKES_PER_OPL)
}

pub fn flake_to_opl(flakes: FlakeAmount) -> u64 {
    flakes / crate::FLAKES_PER_OPL
}

pub fn flakes_to_pennyweight(flakes: FlakeAmount) -> u64 {
    flakes / (crate::FLAKES_PER_OPL / crate::PENNYWEIGHTS_PER_OPL)
}

pub fn flakes_to_grain(flakes: FlakeAmount) -> u64 {
    flakes / (crate::FLAKES_PER_OPL / crate::GRAINS_PER_OPL)
}

pub fn format_flake_as_opl(flakes: u64) -> String {
    let opl = flakes / crate::FLAKES_PER_OPL;
    let frac = flakes % crate::FLAKES_PER_OPL;
    format!("{}.{:06} OPL", opl, frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FLAKES_PER_OPL, GRAINS_PER_OPL, PENNYWEIGHTS_PER_OPL};

    #[test]
    fn hash_zero_is_deterministic() {
        let h1 = Hash::zero();
        let h2 = Hash::zero();
        assert_eq!(h1, h2);
        assert_eq!(h1.0.len(), 32);
    }

    #[test]
    fn opl_flake_roundtrip() {
        assert_eq!(opl_to_flake(1), 1_000_000);
        assert_eq!(flake_to_opl(opl_to_flake(1)), 1);
        assert_eq!(opl_to_flake(0), 0);
    }

    #[test]
    fn pennyweight_conversion() {
        assert_eq!(PENNYWEIGHTS_PER_OPL, 100);
        // 1 OPL = 100 pennyweight, so 1,000,000 flakes / 10,000 flakes per pennyweight = 100
        assert_eq!(flakes_to_pennyweight(FLAKES_PER_OPL), 100);
        assert_eq!(flakes_to_pennyweight(10_000), 1);
    }

    #[test]
    fn grain_conversion() {
        assert_eq!(GRAINS_PER_OPL, 10_000);
        // 1 OPL = 10,000 grains, so 1,000,000 flakes / 100 flakes per grain = 10,000
        assert_eq!(flakes_to_grain(FLAKES_PER_OPL), 10_000);
        assert_eq!(flakes_to_grain(100), 1);
    }

    #[test]
    fn transaction_action_variants() {
        let transfer = TransactionAction::Transfer {
            recipient: ObjectId::zero(),
            amount: 100,
        };
        let bond = TransactionAction::ValidatorBond;
        let unbond = TransactionAction::ValidatorUnbond;
        assert_ne!(transfer, TransactionAction::ValidatorBond);
        assert_ne!(bond, TransactionAction::ValidatorUnbond);
    }

    #[test]
    fn hash_hex_roundtrip() {
        let h = Hash::from_bytes([42u8; 32]);
        let json = serde_json::to_string(&h).unwrap();
        let h2: Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, h2);
        assert_eq!(h.to_hex().len(), 64);
    }

    #[test]
    fn format_flake_amounts() {
        assert_eq!(format_flake_as_opl(1_000_000), "1.000000 OPL");
        assert_eq!(format_flake_as_opl(0), "0.000000 OPL");
        assert_eq!(format_flake_as_opl(1), "0.000001 OPL");
        assert_eq!(format_flake_as_opl(440 * 1_000_000), "440.000000 OPL");
    }
}