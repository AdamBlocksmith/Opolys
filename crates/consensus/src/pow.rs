use opolys_core::{Block, BlockHeader, BlockHeight, Hash, OpolysError};
use opolys_crypto::{Blake3Hasher, hash};

const AUTOLYKOS_DATASET_SIZE: usize = 1 << 20;
const AUTOLYKOS_MIX_ROUNDS: usize = 8;

pub struct AutolykosDataset {
    elements: Vec<[u8; 32]>,
}

impl AutolykosDataset {
    pub fn generate(header_bytes: &[u8], height: BlockHeight) -> Self {
        let size = Self::dataset_size(height);
        let mut elements = Vec::with_capacity(size);

        let mut hasher = Blake3Hasher::new();
        hasher.update(header_bytes);
        hasher.update(&(height.to_be_bytes()));
        let seed = hasher.finalize();

        for i in 0..size {
            let mut element = [0u8; 32];
            let mut h = Blake3Hasher::new();
            h.update(&seed.0);
            h.update(&(i as u64).to_be_bytes());
            let element_hash = h.finalize();
            element.copy_from_slice(&element_hash.0[..32]);
            elements.push(element);
        }

        Self { elements }
    }

    fn dataset_size(height: BlockHeight) -> usize {
        let base = AUTOLYKOS_DATASET_SIZE;
        let growth = (height as usize) / 100_000;
        let doubled = base << growth.min(4);
        doubled.min(1 << 24)
    }

    pub fn get(&self, index: usize) -> Option<&[u8; 32]> {
        self.elements.get(index)
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }
}

pub fn autolykos_hash(
    dataset: &AutolykosDataset,
    header: &BlockHeader,
    nonce: u64,
) -> Hash {
    let mut mixer = [0u8; 32];
    {
        let mut hasher = Blake3Hasher::new();
        hasher.update(&header.previous_hash.0);
        hasher.update(&header.state_root.0);
        hasher.update(&header.transaction_root.0);
        hasher.update(&header.height.to_be_bytes());
        hasher.update(&header.timestamp.to_be_bytes());
        hasher.update(&header.difficulty.to_be_bytes());
        hasher.update(&nonce.to_be_bytes());
        let seed = hasher.finalize();
        mixer.copy_from_slice(&seed.0);
    }

    let ds_size = dataset.len();
    if ds_size == 0 {
        return hash(&mixer);
    }

    for round in 0..AUTOLYKOS_MIX_ROUNDS {
        let idx1 = (u64::from_be_bytes(mixer[..8].try_into().unwrap_or([0u8; 8])) as usize) % ds_size;
        let idx2 = ((u64::from_be_bytes(mixer[8..16].try_into().unwrap_or([0u8; 8])).wrapping_add(round as u64)) as usize) % ds_size;

        let elem1 = dataset.get(idx1).copied().unwrap_or([0u8; 32]);
        let elem2 = dataset.get(idx2).copied().unwrap_or([0u8; 32]);

        let mut hasher = Blake3Hasher::new();
        hasher.update(&mixer);
        hasher.update(&elem1);
        hasher.update(&elem2);
        let mixed = hasher.finalize();
        mixer.copy_from_slice(&mixed.0);
    }

    hash(&mixer)
}

pub fn mine_block(
    header: BlockHeader,
    difficulty: u64,
    max_nonce_attempts: u64,
) -> Option<Block> {
    let header_for_dataset = {
        let mut hasher = Blake3Hasher::new();
        hasher.update(&header.previous_hash.0);
        hasher.update(&header.state_root.0);
        hasher.update(&header.transaction_root.0);
        hasher.update(&header.height.to_be_bytes());
        hasher.update(&header.difficulty.to_be_bytes());
        hasher.finalize().0.to_vec()
    };

    let dataset = AutolykosDataset::generate(&header_for_dataset, header.height);
    let target = u64::MAX / difficulty;

    for nonce in 0..max_nonce_attempts {
        let pow_hash = autolykos_hash(&dataset, &header, nonce);
        let hash_int = u64::from_be_bytes(pow_hash.0[..8].try_into().unwrap_or([0u8; 8]));

        if hash_int < target {
            let mut proof_buf = Vec::with_capacity(8);
            proof_buf.extend_from_slice(&nonce.to_be_bytes());
            let mut header = header;
            header.pow_proof = Some(proof_buf);

            return Some(Block {
                header,
                transactions: vec![],
            });
        }
    }

    None
}

pub fn verify_pow(header: &BlockHeader, difficulty: u64) -> Result<(), OpolysError> {
    let nonce_bytes = header.pow_proof.as_ref()
        .ok_or(OpolysError::InvalidProofOfWork)?;

    if nonce_bytes.len() < 8 {
        return Err(OpolysError::InvalidProofOfWork);
    }

    let nonce = u64::from_be_bytes(nonce_bytes[..8].try_into().map_err(|_| OpolysError::InvalidProofOfWork)?);

    let header_for_dataset = {
        let mut hasher = Blake3Hasher::new();
        hasher.update(&header.previous_hash.0);
        hasher.update(&header.state_root.0);
        hasher.update(&header.transaction_root.0);
        hasher.update(&header.height.to_be_bytes());
        hasher.update(&header.difficulty.to_be_bytes());
        hasher.finalize().0.to_vec()
    };

    let dataset = AutolykosDataset::generate(&header_for_dataset, header.height);
    let pow_hash = autolykos_hash(&dataset, header, nonce);

    let target = u64::MAX / difficulty;
    let hash_int = u64::from_be_bytes(pow_hash.0[..8].try_into().unwrap_or([0u8; 8]));

    if hash_int < target {
        Ok(())
    } else {
        Err(OpolysError::InvalidProofOfWork)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::Hash;

    fn make_header(height: BlockHeight, difficulty: u64) -> BlockHeader {
        BlockHeader {
            height,
            previous_hash: Hash::zero(),
            state_root: Hash::zero(),
            transaction_root: Hash::zero(),
            timestamp: 1000,
            pow_proof: None,
            validator_signature: None,
            difficulty,
        }
    }

    #[test]
    fn test_dataset_generation() {
        let dataset = AutolykosDataset::generate(b"test_header", 0);
        assert!(dataset.len() > 0);
        assert!(dataset.get(0).is_some());
    }

    #[test]
    fn test_mine_and_verify() {
        let header = make_header(1, 1);
        let block = mine_block(header, 1, 10_000_000).unwrap();
        assert!(block.header.pow_proof.is_some());
        assert!(verify_pow(&block.header, 1).is_ok());
    }

    #[test]
    fn test_verify_fails_wrong_difficulty() {
        let header = make_header(1, 1);
        let block = mine_block(header, 1, 10_000_000).unwrap();
        assert!(verify_pow(&block.header, 1_000_000_000).is_err());
    }

    #[test]
    fn test_mining_at_higher_difficulty() {
        let header = make_header(1, 100);
        let block = mine_block(header, 100, 10_000_000);
        if let Some(block) = block {
            assert!(verify_pow(&block.header, 100).is_ok());
        }
    }
}