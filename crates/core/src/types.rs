use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub type FleckAmount = u64;
pub type BlockHeight = u64;
pub type PublicKey = Vec<u8>;
pub type Signature = Vec<u8>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct Hash(pub [u8; 64]);

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as Deserialize>::deserialize(deserializer)?;
        let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom("Hash must be 64 bytes"));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Hash(arr))
    }
}

impl Hash {
    pub fn zero() -> Self {
        Hash([0u8; 64])
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Self {
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
    Transfer { recipient: ObjectId, amount: FleckAmount },
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
    pub fee: FleckAmount,
    pub signature: Vec<u8>,
    pub nonce: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
}

pub fn opl_to_fleck(opl: u64) -> FleckAmount {
    opl.saturating_mul(crate::FLECKS_PER_OPL)
}

pub fn fleck_to_opl(flecks: FleckAmount) -> u64 {
    flecks / crate::FLECKS_PER_OPL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_zero_is_deterministic() {
        let h1 = Hash::zero();
        let h2 = Hash::zero();
        assert_eq!(h1, h2);
    }

    #[test]
    fn opl_fleck_roundtrip() {
        assert_eq!(opl_to_fleck(1), 10_000_000);
        assert_eq!(fleck_to_opl(opl_to_fleck(1)), 1);
        assert_eq!(opl_to_fleck(0), 0);
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
    fn hash_hex_serialization() {
        let h = Hash::zero();
        let hex = h.to_hex();
        assert_eq!(hex.len(), 128);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_serde_roundtrip() {
        let h = Hash::from_bytes([42u8; 64]);
        let json = serde_json::to_string(&h).unwrap();
        let h2: Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, h2);
    }
}