//! EVO-OMAP — Evolving Oriented Memory-hard Algorithm for Proof-of-work
//!
//! This module implements the EVO-OMAP algorithm. Protocol constants are
//! defined in constants.rs. Public specification is in public_spec.rs.

#[cfg(not(target_endian = "little"))]
compile_error!("EVO-OMAP consensus implementation currently supports only little-endian targets");

use std::sync::Arc;

pub use crate::hash::{Hash, blake3_256, blake3_xof, blake3_xof_multi, sha3_256};
pub use crate::public_spec::{
    BRANCH_WAYS_MAX, BRANCH_WAYS_MIN, DOMAIN_BRANCH, DOMAIN_CACHE, DOMAIN_COMMITMENT, DOMAIN_EPOCH,
    DOMAIN_MEMORY, DOMAIN_NODE, DOMAIN_SEED, EPOCH_LENGTH_MAX, EPOCH_LENGTH_MIN, Instruction,
    PROGRAM_LENGTH_MAX, PROGRAM_LENGTH_MIN, Program, STATE_SIZE as STATE_SIZE_SPEC, STEPS_MAX,
    STEPS_MIN,
};

use crate::constants::{
    BRANCH_MASK, BRANCH_NODE_PREFIX, EPOCH_LENGTH, NODE_SIZE, NUM_NODES, NUM_REGISTERS, NUM_STEPS,
    OPERAND_WORDS, PROGRAM_LENGTH, SRC_MASK, STATE_HASH_PREFIX, WRITE_NODE_PREFIX,
};

pub use crate::public_spec::DatasetSpec;

pub fn ensure_little_endian_platform() {
    assert!(
        cfg!(target_endian = "little"),
        "EVO-OMAP consensus implementation currently supports only little-endian targets"
    );
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct State(pub [u8; STATE_SIZE_SPEC]);

impl State {
    pub fn as_u64_array(&self) -> [u64; 8] {
        let mut arr = [0u64; 8];
        for (i, chunk) in self.0.chunks(8).enumerate() {
            arr[i] = u64::from_le_bytes(chunk.try_into().unwrap());
        }
        arr
    }

    pub fn write_u64_at_index(&mut self, index: usize, value: u64) {
        let bytes = &mut self.0[index * 8..index * 8 + 8];
        bytes.copy_from_slice(&value.to_le_bytes());
    }

    pub fn write_all_u64(&mut self, values: &[u64; 8]) {
        for (i, &val) in values.iter().enumerate() {
            self.write_u64_at_index(i, val);
        }
    }

    pub fn as_bytes(&self) -> &[u8; STATE_SIZE_SPEC] {
        &self.0
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8; STATE_SIZE_SPEC] {
        &mut self.0
    }

    pub fn from_seed(seed: &Hash) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(seed.as_ref());
        let mut reader = hasher.finalize_xof();
        let mut output = [0u8; STATE_SIZE_SPEC];
        reader.fill(&mut output);
        State(output)
    }
}

pub trait DatasetLike {
    fn get(&self, index: usize) -> &[u8];
    fn set(&mut self, index: usize, node: Vec<u8>);
    fn as_node_slice(&self) -> Vec<&[u8]>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct Dataset {
    pub nodes: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MemoryMerkleSibling {
    pub hash: Hash,
    pub is_left: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MemoryMerkleProof {
    pub leaf_index: usize,
    pub siblings: Vec<MemoryMerkleSibling>,
}

impl Dataset {
    pub fn new() -> Self {
        Self {
            nodes: vec![Vec::new(); NUM_NODES],
        }
    }

    pub fn get(&self, index: usize) -> &[u8] {
        &self.nodes[index]
    }

    pub fn set(&mut self, index: usize, node: Vec<u8>) {
        self.nodes[index] = node;
    }
}

impl Default for Dataset {
    fn default() -> Self {
        Self::new()
    }
}

impl DatasetLike for Dataset {
    fn get(&self, index: usize) -> &[u8] {
        &self.nodes[index]
    }

    fn set(&mut self, index: usize, node: Vec<u8>) {
        self.nodes[index] = node;
    }

    fn as_node_slice(&self) -> Vec<&[u8]> {
        self.nodes.iter().map(|n| n.as_slice()).collect()
    }
}

pub struct CowDataset<'a> {
    base: &'a Dataset,
    modified: Vec<Option<Vec<u8>>>,
    modified_indices: Vec<usize>,
}

impl<'a> CowDataset<'a> {
    pub fn new(base: &'a Dataset) -> Self {
        Self {
            base,
            modified: vec![None; NUM_NODES],
            modified_indices: Vec::new(),
        }
    }

    pub fn get(&self, index: usize) -> &[u8] {
        if let Some(ref node) = self.modified[index] {
            node
        } else {
            &self.base.nodes[index]
        }
    }

    pub fn set(&mut self, index: usize, node: Vec<u8>) {
        if self.modified[index].is_none() {
            self.modified_indices.push(index);
        }
        self.modified[index] = Some(node);
    }

    pub fn reset(&mut self) {
        for &i in &self.modified_indices {
            self.modified[i] = None;
        }
        self.modified_indices.clear();
    }

    pub fn as_dataset(&self) -> Vec<&[u8]> {
        let mut result = Vec::with_capacity(NUM_NODES);
        for i in 0..NUM_NODES {
            result.push(self.get(i));
        }
        result
    }

    pub fn as_node_slice(&self) -> Vec<&[u8]> {
        self.as_dataset()
    }
}

impl<'a> DatasetLike for CowDataset<'a> {
    fn get(&self, index: usize) -> &[u8] {
        CowDataset::get(self, index)
    }

    fn set(&mut self, index: usize, node: Vec<u8>) {
        CowDataset::set(self, index, node);
    }

    fn as_node_slice(&self) -> Vec<&[u8]> {
        self.as_dataset()
    }
}

pub struct LightDataset {
    epoch_seed: Hash,
    original_nodes: Vec<Option<Vec<u8>>>,
    modified_nodes: Vec<Option<Vec<u8>>>,
}

impl LightDataset {
    pub fn new(epoch_seed: &Hash) -> Self {
        Self {
            epoch_seed: *epoch_seed,
            original_nodes: vec![None; NUM_NODES],
            modified_nodes: vec![None; NUM_NODES],
        }
    }

    pub fn reset(&mut self) {
        self.original_nodes = vec![None; NUM_NODES];
        self.modified_nodes = vec![None; NUM_NODES];
    }

    fn reconstruct_node_raw(&self, prev_node: &[u8], index: usize) -> Vec<u8> {
        let index_bytes = (index as u64).to_le_bytes();
        let epoch_seed_bytes = self.epoch_seed.as_ref();
        let mut data = Vec::with_capacity(48 + NODE_SIZE);
        data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
        data.extend_from_slice(epoch_seed_bytes);
        data.extend_from_slice(prev_node);
        data.extend_from_slice(&index_bytes);
        blake3_xof(&data, NODE_SIZE)
    }

    fn get_original_chain_node(&mut self, index: usize) -> Vec<u8> {
        if self.original_nodes[index].is_none() {
            let prev_node = if index == 0 {
                Vec::new()
            } else {
                self.get_original_chain_node(index - 1)
            };
            let node = self.reconstruct_node_raw(&prev_node, index);
            self.original_nodes[index] = Some(node);
        }
        self.original_nodes[index].as_ref().unwrap().clone()
    }

    pub fn get_node(&mut self, index: usize) -> Vec<u8> {
        if let Some(ref node) = self.modified_nodes[index] {
            return node.clone();
        }
        self.get_original_chain_node(index)
    }

    pub fn set_node(&mut self, index: usize, node: Vec<u8>) {
        assert_eq!(
            node.len(),
            NODE_SIZE,
            "node must be exactly NODE_SIZE bytes"
        );
        self.modified_nodes[index] = Some(node);
    }

    pub fn compute_memory_commitment(&mut self) -> Hash {
        let mut leaves = Vec::with_capacity(NUM_NODES);
        for i in 0..NUM_NODES {
            let node = self.get_node(i);
            leaves.push(compute_memory_leaf_hash(i, &node));
        }
        compute_memory_merkle_root_from_leaves(leaves)
    }
}

// Returns a length-prefixed copy of a domain separator byte string.
// The first byte is the domain length; the rest is the domain itself.
fn prefixed_domain(domain: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + domain.len());
    v.push(domain.len() as u8);
    v.extend_from_slice(domain);
    v
}

pub fn evo_omap_hash_light(
    dataset: &mut LightDataset,
    header: &[u8],
    height: u64,
    nonce: u64,
) -> Hash {
    let seed = compute_mining_seed(header, height, nonce);
    let mut state = State::from_seed(&seed);

    let mut commitment_data = Vec::with_capacity(40);
    commitment_data.extend_from_slice(&prefixed_domain(DOMAIN_COMMITMENT));
    commitment_data.extend_from_slice(&height.to_le_bytes());
    let mut commitment_hash = blake3_256(&commitment_data);

    for step in 0..NUM_STEPS {
        let program = generate_program(&state);
        let (idx1, idx2, idx_write) = derive_indices(&state, step as u64);

        let node1 = dataset.get_node(idx1);
        let node2 = dataset.get_node(idx2);

        execute_program(&mut state, &program, &node1, &node2);

        apply_branch(&mut state, step as u32, &node1, &node2);

        let state_bytes = state.as_bytes();
        let mut write_data = Vec::with_capacity(WRITE_NODE_PREFIX * 2 + STATE_HASH_PREFIX);
        write_data.extend_from_slice(&node1[..WRITE_NODE_PREFIX]);
        write_data.extend_from_slice(&node2[..WRITE_NODE_PREFIX]);
        write_data.extend_from_slice(&state_bytes[..STATE_HASH_PREFIX]);
        let written = blake3_xof(&write_data, NODE_SIZE);
        dataset.set_node(idx_write, written);

        commitment_hash = blake3_256(
            &commitment_hash
                .as_ref()
                .iter()
                .chain(&(step as u64).to_le_bytes())
                .chain(&state_bytes[..32])
                .cloned()
                .collect::<Vec<u8>>(),
        );
    }

    let state_summary = blake3_256(state.as_bytes());
    let memory_commitment = dataset.compute_memory_commitment();
    let final_input: Vec<u8> = state_summary
        .as_ref()
        .iter()
        .chain(commitment_hash.as_ref())
        .chain(memory_commitment.as_ref())
        .cloned()
        .collect();

    sha3_256(&final_input)
}

pub fn compute_epoch_number_with_epoch_length(height: u64, epoch_length: u64) -> u64 {
    assert!(epoch_length > 0, "epoch length must be non-zero");
    height / epoch_length
}

pub fn compute_epoch_seed(height: u64) -> Hash {
    compute_epoch_seed_with_epoch_length(height, EPOCH_LENGTH)
}

pub fn compute_epoch_seed_with_epoch_length(height: u64, epoch_length: u64) -> Hash {
    compute_epoch_seed_with_epoch_length_and_seed_material(height, epoch_length, &[])
}

pub fn compute_epoch_seed_with_epoch_length_and_seed_material(
    height: u64,
    epoch_length: u64,
    seed_material: &[u8],
) -> Hash {
    let epoch = compute_epoch_number_with_epoch_length(height, epoch_length);
    let mut data = Vec::with_capacity(24 + seed_material.len());
    data.extend_from_slice(&prefixed_domain(DOMAIN_EPOCH));
    data.extend_from_slice(&epoch.to_le_bytes());
    data.extend_from_slice(&(seed_material.len() as u64).to_le_bytes());
    data.extend_from_slice(seed_material);
    blake3_256(&data)
}

fn compute_mining_seed(header: &[u8], height: u64, nonce: u64) -> Hash {
    let header_commitment = blake3_256(header);
    let header = header_commitment.as_ref();
    let mut data = Vec::with_capacity(48 + header.len());
    data.extend_from_slice(&prefixed_domain(DOMAIN_SEED));
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header);
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&nonce.to_le_bytes());
    blake3_256(&data)
}

fn generate_node0(seed: &Hash) -> Vec<u8> {
    let epoch_seed_bytes = seed.as_ref();
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
    data.extend_from_slice(epoch_seed_bytes);
    data.extend_from_slice(&0u64.to_le_bytes());
    blake3_xof(&data, NODE_SIZE)
}

pub fn generate_dataset(seed: &Hash) -> Dataset {
    let mut dataset = Dataset::new();
    let epoch_seed_bytes = seed.as_ref();

    dataset.nodes[0] = generate_node0(seed);

    for i in 1..NUM_NODES {
        let mut data = Vec::with_capacity(48 + NODE_SIZE);
        data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
        data.extend_from_slice(epoch_seed_bytes);
        data.extend_from_slice(&dataset.nodes[i - 1]);
        data.extend_from_slice(&(i as u64).to_le_bytes());
        dataset.nodes[i] = blake3_xof(&data, NODE_SIZE);
    }

    dataset
}

pub fn generate_program(state: &State) -> Program {
    let words = state.as_u64_array();
    let mut instructions = Vec::with_capacity(PROGRAM_LENGTH);

    for i in 0..PROGRAM_LENGTH {
        let word_idx = i % NUM_REGISTERS;
        // Mix a position-dependent constant into the word so that slots sharing
        // the same word_idx (e.g. i=0 and i=8) produce distinct selectors.
        // The constant is the 64-bit golden-ratio multiplier (Knuth, TAOCP).
        let selector = words[word_idx].wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15));

        let op_bits = selector & 0x07;
        let dst = ((selector >> 3) & 0x07) as u8;
        let src = ((selector >> 6) & SRC_MASK) as u8;

        let instruction = match op_bits {
            0 => Instruction::Add { dst, src },
            1 => Instruction::Sub { dst, src },
            2 => Instruction::Mul { dst, src },
            3 => Instruction::Xor { dst, src },
            4 => Instruction::Rotl { dst, src },
            5 => Instruction::Rotr { dst, src },
            6 => Instruction::Mulh { dst, src },
            7 => Instruction::Swap { a: dst, b: src },
            _ => unreachable!(),
        };
        instructions.push(instruction);
    }

    Program { instructions }
}

fn fill_program_buffer(words: &[u64; 8], buf: &mut Vec<Instruction>) {
    buf.clear();
    for i in 0..PROGRAM_LENGTH {
        let word_idx = i % NUM_REGISTERS;
        let selector = words[word_idx].wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15));
        let op_bits = selector & 0x07;
        let dst = ((selector >> 3) & 0x07) as u8;
        let src = ((selector >> 6) & SRC_MASK) as u8;
        let instruction = match op_bits {
            0 => Instruction::Add { dst, src },
            1 => Instruction::Sub { dst, src },
            2 => Instruction::Mul { dst, src },
            3 => Instruction::Xor { dst, src },
            4 => Instruction::Rotl { dst, src },
            5 => Instruction::Rotr { dst, src },
            6 => Instruction::Mulh { dst, src },
            7 => Instruction::Swap { a: dst, b: src },
            _ => unreachable!(),
        };
        buf.push(instruction);
    }
}

fn execute_instructions(
    state: &mut State,
    instructions: &[Instruction],
    node1: &[u8],
    node2: &[u8],
) {
    let node1_words = node_as_u64_array(node1);
    let node2_words = node_as_u64_array(node2);
    let mut state_arr = state.as_u64_array();
    for instruction in instructions {
        instruction.execute(&mut state_arr, &node1_words, &node2_words);
    }
    state.write_all_u64(&state_arr);
}

pub fn derive_indices(state: &State, step: u64) -> (usize, usize, usize) {
    let words = state.as_u64_array();
    derive_indices_from_words(&words, step)
}

fn derive_indices_from_words(words: &[u64; 8], step: u64) -> (usize, usize, usize) {
    let idx1 = (words[0]
        .wrapping_add(step)
        .wrapping_mul(words[4].wrapping_add(1))
        % NUM_NODES as u64) as usize;
    let idx2 = (words[1]
        .wrapping_mul(step.wrapping_add(1))
        .wrapping_add(words[5])
        % NUM_NODES as u64) as usize;
    let idx_write = ((words[2] ^ words[3] ^ step) % NUM_NODES as u64) as usize;
    (idx1, idx2, idx_write)
}

fn node_as_u64_array(node: &[u8]) -> Vec<u64> {
    let word_count = OPERAND_WORDS;
    let mut words = Vec::with_capacity(word_count);
    for i in 0..word_count {
        let bytes = &node[i * 8..i * 8 + 8];
        words.push(u64::from_le_bytes(bytes.try_into().unwrap()));
    }
    words
}

pub fn execute_program(state: &mut State, program: &Program, node1: &[u8], node2: &[u8]) {
    let node1_words = node_as_u64_array(node1);
    let node2_words = node_as_u64_array(node2);
    let mut state_arr = state.as_u64_array();

    for instruction in &program.instructions {
        instruction.execute(&mut state_arr, &node1_words, &node2_words);
    }

    state.write_all_u64(&state_arr);
}

pub fn apply_branch(state: &mut State, step: u32, node1: &[u8], node2: &[u8]) {
    let words = state.as_u64_array();
    let branch_variant = (words[0] & BRANCH_MASK) as u8;
    let state_bytes = state.as_bytes();

    let mut input = Vec::with_capacity(16 + 32 + 64);
    input.extend_from_slice(&prefixed_domain(DOMAIN_BRANCH));
    input.extend_from_slice(&step.to_le_bytes());
    input.push(branch_variant);
    match branch_variant {
        0 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
        }
        1 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
        }
        2 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
        }
        3 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
        }
        _ => unreachable!(),
    }

    let output = blake3_xof(&input, STATE_SIZE_SPEC);
    let mut state_arr = state.as_u64_array();
    for i in 0..NUM_REGISTERS {
        let xor_val = u64::from_le_bytes(output[i * 8..(i + 1) * 8].try_into().unwrap());
        state_arr[i] ^= xor_val;
    }
    state.write_all_u64(&state_arr);
}

pub fn compute_memory_commitment(dataset: &Dataset) -> Hash {
    let nodes = dataset.as_node_slice();
    compute_memory_commitment_from_slice(&nodes)
}

pub fn compute_memory_commitment_from_slice(nodes: &[&[u8]]) -> Hash {
    let leaves = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| compute_memory_leaf_hash(index, node))
        .collect();
    compute_memory_merkle_root_from_leaves(leaves)
}

pub fn build_memory_merkle_proof_from_slice(
    nodes: &[&[u8]],
    leaf_index: usize,
) -> Option<MemoryMerkleProof> {
    if leaf_index >= nodes.len() {
        return None;
    }

    let mut index = leaf_index;
    let mut level: Vec<Hash> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| compute_memory_leaf_hash(index, node))
        .collect();
    let mut siblings = Vec::new();

    while level.len() > 1 {
        let sibling_index = if index % 2 == 0 {
            (index + 1).min(level.len() - 1)
        } else {
            index - 1
        };
        siblings.push(MemoryMerkleSibling {
            hash: level[sibling_index],
            is_left: sibling_index < index,
        });

        level = merkle_parent_level(&level);
        index /= 2;
    }

    Some(MemoryMerkleProof {
        leaf_index,
        siblings,
    })
}

pub fn build_memory_merkle_proof(
    dataset: &Dataset,
    leaf_index: usize,
) -> Option<MemoryMerkleProof> {
    let nodes = dataset.as_node_slice();
    build_memory_merkle_proof_from_slice(&nodes, leaf_index)
}

pub fn verify_memory_merkle_proof(root: &Hash, node: &[u8], proof: &MemoryMerkleProof) -> bool {
    let mut current = compute_memory_leaf_hash(proof.leaf_index, node);
    for sibling in &proof.siblings {
        current = if sibling.is_left {
            compute_memory_parent_hash(&sibling.hash, &current)
        } else {
            compute_memory_parent_hash(&current, &sibling.hash)
        };
    }
    &current == root
}

fn compute_memory_leaf_hash(index: usize, node: &[u8]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&prefixed_domain(DOMAIN_MEMORY));
    hasher.update(b"leaf");
    hasher.update(&(index as u64).to_le_bytes());
    hasher.update(&(node.len() as u64).to_le_bytes());
    hasher.update(node);
    let result = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(result.as_bytes());
    Hash(arr)
}

fn compute_memory_parent_hash(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&prefixed_domain(DOMAIN_MEMORY));
    hasher.update(b"parent");
    hasher.update(left.as_ref());
    hasher.update(right.as_ref());
    let result = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(result.as_bytes());
    Hash(arr)
}

fn compute_memory_merkle_root_from_leaves(leaves: Vec<Hash>) -> Hash {
    assert!(
        !leaves.is_empty(),
        "memory commitment requires at least one node"
    );
    let mut level = leaves;
    while level.len() > 1 {
        level = merkle_parent_level(&level);
    }
    level[0]
}

fn merkle_parent_level(level: &[Hash]) -> Vec<Hash> {
    level
        .chunks(2)
        .map(|pair| {
            let left = pair[0];
            let right = *pair.get(1).unwrap_or(&left);
            compute_memory_parent_hash(&left, &right)
        })
        .collect()
}

pub fn evo_omap_hash<D: DatasetLike>(
    dataset: &mut D,
    header: &[u8],
    height: u64,
    nonce: u64,
) -> Hash {
    let seed = compute_mining_seed(header, height, nonce);
    let mut state = State::from_seed(&seed);

    let mut commitment_data = Vec::with_capacity(40);
    commitment_data.extend_from_slice(&prefixed_domain(DOMAIN_COMMITMENT));
    commitment_data.extend_from_slice(&height.to_le_bytes());
    let mut commitment_hash = blake3_256(&commitment_data);

    for step in 0..NUM_STEPS {
        let program = generate_program(&state);
        let (idx1, idx2, idx_write) = derive_indices(&state, step as u64);

        let node1 = dataset.get(idx1).to_vec();
        let node2 = dataset.get(idx2).to_vec();

        execute_program(&mut state, &program, &node1, &node2);

        apply_branch(&mut state, step as u32, &node1, &node2);

        let state_bytes = state.as_bytes();
        let mut write_data = Vec::with_capacity(WRITE_NODE_PREFIX * 2 + STATE_HASH_PREFIX);
        write_data.extend_from_slice(&node1[..WRITE_NODE_PREFIX]);
        write_data.extend_from_slice(&node2[..WRITE_NODE_PREFIX]);
        write_data.extend_from_slice(&state_bytes[..STATE_HASH_PREFIX]);
        let written = blake3_xof(&write_data, NODE_SIZE);
        dataset.set(idx_write, written);

        commitment_hash = blake3_256(
            &commitment_hash
                .as_ref()
                .iter()
                .chain(&(step as u64).to_le_bytes())
                .chain(&state_bytes[..32])
                .cloned()
                .collect::<Vec<u8>>(),
        );
    }

    let state_summary = blake3_256(state.as_bytes());
    let nodes = dataset.as_node_slice();
    let memory_commitment = compute_memory_commitment_from_slice(&nodes);
    let final_input: Vec<u8> = state_summary
        .as_ref()
        .iter()
        .chain(commitment_hash.as_ref())
        .chain(memory_commitment.as_ref())
        .cloned()
        .collect();

    sha3_256(&final_input)
}

pub struct HashBuffers {
    pub commitment_data: Vec<u8>,
    pub write_data: Vec<u8>,
    pub branch_input: Vec<u8>,
    pub commitment_input: Vec<u8>,
    pub final_input: Vec<u8>,
    pub program_buf: Vec<Instruction>,
    pub node1_prefix: Vec<u8>,
    pub node2_prefix: Vec<u8>,
}

impl HashBuffers {
    pub fn new() -> Self {
        Self {
            commitment_data: Vec::with_capacity(40),
            write_data: Vec::with_capacity(WRITE_NODE_PREFIX * 2 + STATE_HASH_PREFIX),
            branch_input: Vec::with_capacity(16 + 32 + 64 + 32 + 32),
            commitment_input: Vec::with_capacity(32 + 8 + 32),
            final_input: Vec::with_capacity(32 + 32 + 32),
            program_buf: Vec::with_capacity(PROGRAM_LENGTH),
            node1_prefix: Vec::with_capacity(WRITE_NODE_PREFIX),
            node2_prefix: Vec::with_capacity(WRITE_NODE_PREFIX),
        }
    }

    pub fn reset(&mut self) {
        self.commitment_data.clear();
        self.write_data.clear();
        self.branch_input.clear();
        self.commitment_input.clear();
        self.final_input.clear();
        self.program_buf.clear();
        self.node1_prefix.clear();
        self.node2_prefix.clear();
    }
}

impl Default for HashBuffers {
    fn default() -> Self {
        Self::new()
    }
}

pub fn evo_omap_hash_with_buffers<D: DatasetLike>(
    dataset: &mut D,
    header: &[u8],
    height: u64,
    nonce: u64,
    buffers: &mut HashBuffers,
) -> Hash {
    let seed = compute_mining_seed(header, height, nonce);
    let mut state = State::from_seed(&seed);

    buffers.commitment_data.clear();
    buffers
        .commitment_data
        .extend_from_slice(&prefixed_domain(DOMAIN_COMMITMENT));
    buffers
        .commitment_data
        .extend_from_slice(&height.to_le_bytes());
    let mut commitment_hash = blake3_256(&buffers.commitment_data);

    for step in 0..NUM_STEPS {
        let state_words = state.as_u64_array();
        fill_program_buffer(&state_words, &mut buffers.program_buf);
        let (idx1, idx2, idx_write) = derive_indices_from_words(&state_words, step as u64);

        {
            let s = dataset.get(idx1);
            buffers.node1_prefix.clear();
            buffers
                .node1_prefix
                .extend_from_slice(&s[..WRITE_NODE_PREFIX]);
        }
        {
            let s = dataset.get(idx2);
            buffers.node2_prefix.clear();
            buffers
                .node2_prefix
                .extend_from_slice(&s[..WRITE_NODE_PREFIX]);
        }

        execute_instructions(
            &mut state,
            &buffers.program_buf,
            &buffers.node1_prefix,
            &buffers.node2_prefix,
        );

        apply_branch_with_buffer(
            &mut state,
            step as u32,
            &buffers.node1_prefix,
            &buffers.node2_prefix,
            &mut buffers.branch_input,
        );

        let state_bytes = state.as_bytes();
        buffers.write_data.clear();
        buffers.write_data.extend_from_slice(&buffers.node1_prefix);
        buffers.write_data.extend_from_slice(&buffers.node2_prefix);
        buffers
            .write_data
            .extend_from_slice(&state_bytes[..STATE_HASH_PREFIX]);
        let written = blake3_xof(&buffers.write_data, NODE_SIZE);
        dataset.set(idx_write, written);

        buffers.commitment_input.clear();
        buffers
            .commitment_input
            .extend_from_slice(commitment_hash.as_ref());
        buffers
            .commitment_input
            .extend_from_slice(&(step as u64).to_le_bytes());
        buffers
            .commitment_input
            .extend_from_slice(&state_bytes[..32]);
        commitment_hash = blake3_256(&buffers.commitment_input);
    }

    let state_summary = blake3_256(state.as_bytes());
    let nodes = dataset.as_node_slice();
    let memory_commitment = compute_memory_commitment_from_slice(&nodes);
    buffers.final_input.clear();
    buffers
        .final_input
        .extend_from_slice(state_summary.as_ref());
    buffers
        .final_input
        .extend_from_slice(commitment_hash.as_ref());
    buffers
        .final_input
        .extend_from_slice(memory_commitment.as_ref());

    sha3_256(&buffers.final_input)
}

pub fn apply_branch_with_buffer(
    state: &mut State,
    step: u32,
    node1: &[u8],
    node2: &[u8],
    input: &mut Vec<u8>,
) {
    let mut state_arr = state.as_u64_array();
    let branch_variant = (state_arr[0] & BRANCH_MASK) as u8;
    let state_bytes = state.as_bytes();

    input.clear();
    input.extend_from_slice(&prefixed_domain(DOMAIN_BRANCH));
    input.extend_from_slice(&step.to_le_bytes());
    input.push(branch_variant);
    match branch_variant {
        0 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
        }
        1 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
        }
        2 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
        }
        3 => {
            input.extend_from_slice(state_bytes);
            input.extend_from_slice(&node2[..BRANCH_NODE_PREFIX]);
            input.extend_from_slice(&node1[..BRANCH_NODE_PREFIX]);
        }
        _ => unreachable!(),
    }

    let output = blake3_xof(input, STATE_SIZE_SPEC);
    for i in 0..NUM_REGISTERS {
        let xor_val = u64::from_le_bytes(output[i * 8..(i + 1) * 8].try_into().unwrap());
        state_arr[i] ^= xor_val;
    }
    state.write_all_u64(&state_arr);
}

pub fn mine(
    header: &[u8],
    height: u64,
    difficulty: u64,
    max_nonce_attempts: u64,
) -> (Option<u64>, u64) {
    mine_with_epoch_length(header, height, difficulty, max_nonce_attempts, EPOCH_LENGTH)
}

pub fn mine_with_epoch_length(
    header: &[u8],
    height: u64,
    difficulty: u64,
    max_nonce_attempts: u64,
    epoch_length: u64,
) -> (Option<u64>, u64) {
    mine_with_epoch_length_and_seed_material(
        header,
        height,
        difficulty,
        max_nonce_attempts,
        epoch_length,
        &[],
    )
}

pub fn mine_with_epoch_length_and_seed_material(
    header: &[u8],
    height: u64,
    difficulty: u64,
    max_nonce_attempts: u64,
    epoch_length: u64,
    seed_material: &[u8],
) -> (Option<u64>, u64) {
    if difficulty == 0 {
        return (None, 0);
    }
    let epoch_seed =
        compute_epoch_seed_with_epoch_length_and_seed_material(height, epoch_length, seed_material);
    let base_dataset = generate_dataset(&epoch_seed);
    let mut cow_dataset = CowDataset::new(&base_dataset);
    let mut buffers = HashBuffers::new();
    let mut attempts = 0u64;

    for nonce in 0..max_nonce_attempts {
        cow_dataset.reset();
        attempts += 1;

        let pow_hash =
            evo_omap_hash_with_buffers(&mut cow_dataset, header, height, nonce, &mut buffers);

        let leading_zeros = pow_hash
            .0
            .iter()
            .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
            .take_while(|&b| b == 0)
            .count() as u64;
        if leading_zeros >= difficulty {
            return (Some(nonce), attempts);
        }
    }

    (None, attempts)
}

use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub fn mine_parallel(
    header: &[u8],
    height: u64,
    difficulty: u64,
    max_nonce_attempts: u64,
    num_threads: usize,
) -> (Option<u64>, u64) {
    mine_parallel_with_epoch_length(
        header,
        height,
        difficulty,
        max_nonce_attempts,
        num_threads,
        EPOCH_LENGTH,
    )
}

pub fn mine_parallel_with_epoch_length(
    header: &[u8],
    height: u64,
    difficulty: u64,
    max_nonce_attempts: u64,
    num_threads: usize,
    epoch_length: u64,
) -> (Option<u64>, u64) {
    mine_parallel_with_epoch_length_and_seed_material(
        header,
        height,
        difficulty,
        max_nonce_attempts,
        num_threads,
        epoch_length,
        &[],
    )
}

pub fn mine_parallel_with_epoch_length_and_seed_material(
    header: &[u8],
    height: u64,
    difficulty: u64,
    max_nonce_attempts: u64,
    num_threads: usize,
    epoch_length: u64,
    seed_material: &[u8],
) -> (Option<u64>, u64) {
    if difficulty == 0 {
        return (None, 0);
    }
    let num_threads = if num_threads == 0 {
        rayon::current_num_threads()
    } else {
        num_threads
    };
    let epoch_seed =
        compute_epoch_seed_with_epoch_length_and_seed_material(height, epoch_length, seed_material);
    let base_dataset = Arc::new(generate_dataset(&epoch_seed));
    let header = Arc::new(header.to_vec());

    let nonce_counter = Arc::new(AtomicU64::new(0));
    let found_flag = Arc::new(AtomicBool::new(false));
    let found_nonce = Arc::new(AtomicU64::new(u64::MAX));
    let total_attempts = Arc::new(AtomicU64::new(0));

    (0..num_threads).into_par_iter().for_each(|_| {
        let mut buffers = HashBuffers::new();
        let mut cow = CowDataset::new(&base_dataset);

        loop {
            if found_flag.load(Ordering::Acquire) {
                break;
            }

            let nonce = nonce_counter.fetch_add(1, Ordering::Relaxed);
            if nonce >= max_nonce_attempts {
                break;
            }

            total_attempts.fetch_add(1, Ordering::Relaxed);
            cow.reset();

            let pow_hash =
                evo_omap_hash_with_buffers(&mut cow, &header, height, nonce, &mut buffers);

            let leading_zeros = pow_hash
                .0
                .iter()
                .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
                .take_while(|&b| b == 0)
                .count() as u64;

            if leading_zeros >= difficulty {
                found_flag.store(true, Ordering::Release);
                let _ = found_nonce.compare_exchange(
                    u64::MAX,
                    nonce,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                break;
            }
        }
    });

    let attempts = total_attempts.load(Ordering::Relaxed);
    if found_flag.load(Ordering::Acquire) {
        (Some(found_nonce.load(Ordering::Relaxed)), attempts)
    } else {
        (None, attempts)
    }
}

pub struct DatasetCache {
    epoch: Option<u64>,
    seed_material_hash: Option<Hash>,
    dataset: Dataset,
}

impl DatasetCache {
    pub fn new() -> Self {
        Self {
            epoch: None,
            seed_material_hash: None,
            dataset: Dataset::new(),
        }
    }

    pub fn get_dataset(&mut self, height: u64) -> &Dataset {
        self.get_dataset_with_epoch_length(height, EPOCH_LENGTH)
    }

    pub fn get_dataset_with_epoch_length(&mut self, height: u64, epoch_length: u64) -> &Dataset {
        self.get_dataset_with_epoch_length_and_seed_material(height, epoch_length, &[])
    }

    pub fn get_dataset_with_epoch_length_and_seed_material(
        &mut self,
        height: u64,
        epoch_length: u64,
        seed_material: &[u8],
    ) -> &Dataset {
        let epoch = compute_epoch_number_with_epoch_length(height, epoch_length);
        let seed_material_hash = blake3_256(seed_material);
        if self.epoch != Some(epoch) || self.seed_material_hash != Some(seed_material_hash) {
            let epoch_seed = compute_epoch_seed_with_epoch_length_and_seed_material(
                height,
                epoch_length,
                seed_material,
            );
            self.dataset = generate_dataset(&epoch_seed);
            self.epoch = Some(epoch);
            self.seed_material_hash = Some(seed_material_hash);
        }
        &self.dataset
    }
}

impl Default for DatasetCache {
    fn default() -> Self {
        Self::new()
    }
}

pub fn verify(header: &[u8], height: u64, nonce: u64, difficulty: u64) -> bool {
    verify_with_epoch_length(header, height, nonce, difficulty, EPOCH_LENGTH)
}

pub fn verify_with_epoch_length(
    header: &[u8],
    height: u64,
    nonce: u64,
    difficulty: u64,
    epoch_length: u64,
) -> bool {
    verify_with_epoch_length_and_seed_material(header, height, nonce, difficulty, epoch_length, &[])
}

pub fn verify_with_epoch_length_and_seed_material(
    header: &[u8],
    height: u64,
    nonce: u64,
    difficulty: u64,
    epoch_length: u64,
    seed_material: &[u8],
) -> bool {
    if difficulty == 0 {
        return false;
    }
    let epoch_seed =
        compute_epoch_seed_with_epoch_length_and_seed_material(height, epoch_length, seed_material);
    let mut dataset = generate_dataset(&epoch_seed);
    let pow_hash = evo_omap_hash(&mut dataset, header, height, nonce);
    pow_hash
        .0
        .iter()
        .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
        .take_while(|&b| b == 0)
        .count() as u64
        >= difficulty
}

pub fn verify_light(header: &[u8], height: u64, nonce: u64, difficulty: u64) -> bool {
    verify_light_with_epoch_length(header, height, nonce, difficulty, EPOCH_LENGTH)
}

pub fn verify_light_with_epoch_length(
    header: &[u8],
    height: u64,
    nonce: u64,
    difficulty: u64,
    epoch_length: u64,
) -> bool {
    verify_light_with_epoch_length_and_seed_material(
        header,
        height,
        nonce,
        difficulty,
        epoch_length,
        &[],
    )
}

pub fn verify_light_with_epoch_length_and_seed_material(
    header: &[u8],
    height: u64,
    nonce: u64,
    difficulty: u64,
    epoch_length: u64,
    seed_material: &[u8],
) -> bool {
    if difficulty == 0 {
        return false;
    }
    let epoch_seed =
        compute_epoch_seed_with_epoch_length_and_seed_material(height, epoch_length, seed_material);
    let mut dataset = LightDataset::new(&epoch_seed);
    dataset.reset();

    let pow_hash = evo_omap_hash_light(&mut dataset, header, height, nonce);
    pow_hash
        .0
        .iter()
        .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
        .take_while(|&b| b == 0)
        .count() as u64
        >= difficulty
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // 1. DETERMINISM TESTS
    // =============================================================================

    #[test]
    fn consensus_platform_is_little_endian() {
        ensure_little_endian_platform();
        assert!(cfg!(target_endian = "little"));
    }

    #[test]
    fn compute_epoch_seed_with_epoch_length_uses_custom_boundary() {
        let default_seed_0 = compute_epoch_seed(0);
        assert_eq!(default_seed_0, compute_epoch_seed(1023));
        assert_ne!(default_seed_0, compute_epoch_seed(1024));

        let custom_seed_0 = compute_epoch_seed_with_epoch_length(0, 960);
        assert_eq!(
            custom_seed_0,
            compute_epoch_seed_with_epoch_length(959, 960)
        );
        assert_ne!(
            custom_seed_0,
            compute_epoch_seed_with_epoch_length(960, 960)
        );
        assert_eq!(
            compute_epoch_seed_with_epoch_length(960, 960),
            compute_epoch_seed_with_epoch_length(1023, 960)
        );
    }

    #[test]
    fn compute_epoch_seed_includes_seed_material() {
        let seed_a = compute_epoch_seed_with_epoch_length_and_seed_material(960, 960, b"parent-a");
        let seed_b = compute_epoch_seed_with_epoch_length_and_seed_material(960, 960, b"parent-b");
        let seed_empty = compute_epoch_seed_with_epoch_length(960, 960);

        assert_ne!(seed_a, seed_b);
        assert_ne!(seed_a, seed_empty);
    }

    #[test]
    fn dataset_cache_respects_custom_epoch_length() {
        let mut cache = DatasetCache::new();

        let node_at_0 = cache.get_dataset_with_epoch_length(0, 960).get(0).to_vec();
        let node_at_959 = cache
            .get_dataset_with_epoch_length(959, 960)
            .get(0)
            .to_vec();
        let node_at_960 = cache
            .get_dataset_with_epoch_length(960, 960)
            .get(0)
            .to_vec();

        assert_eq!(node_at_0, node_at_959);
        assert_ne!(node_at_0, node_at_960);
    }

    #[test]
    fn dataset_cache_respects_seed_material() {
        let mut cache = DatasetCache::new();

        let node_parent_a = cache
            .get_dataset_with_epoch_length_and_seed_material(960, 960, b"parent-a")
            .get(0)
            .to_vec();
        let node_parent_a_again = cache
            .get_dataset_with_epoch_length_and_seed_material(1023, 960, b"parent-a")
            .get(0)
            .to_vec();
        let node_parent_b = cache
            .get_dataset_with_epoch_length_and_seed_material(1023, 960, b"parent-b")
            .get(0)
            .to_vec();

        assert_eq!(node_parent_a, node_parent_a_again);
        assert_ne!(node_parent_a, node_parent_b);
    }

    #[test]
    fn test_determinism_same_inputs_same_hash() {
        let header = b"determinism test header";
        let height = 100u64;
        let nonce = 42u64;

        let seed = compute_epoch_seed(height);
        let mut ds1 = generate_dataset(&seed);
        let mut ds2 = generate_dataset(&seed);

        let hash1 = evo_omap_hash(&mut ds1, header, height, nonce);
        let hash2 = evo_omap_hash(&mut ds2, header, height, nonce);

        assert_eq!(hash1, hash2, "Same inputs must produce identical hash");
    }

    #[test]
    fn test_determinism_fuzz_50_random_combinations() {
        let mut rng_state: u64 = 0x1234567890ABCDEF;

        for _ in 0..50 {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);

            let _seed_bytes: [u8; 1] = (rng_state as u8).to_le_bytes();

            let seed = compute_epoch_seed(rng_state % 10000);

            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let nonce = rng_state;

            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let height = rng_state % 10000;

            let header = format!("header_{}", rng_state % 1000);

            let mut ds1 = generate_dataset(&seed);
            let mut ds2 = generate_dataset(&seed);
            let h1 = evo_omap_hash(&mut ds1, header.as_bytes(), height, nonce);
            let h2 = evo_omap_hash(&mut ds2, header.as_bytes(), height, nonce);

            assert_eq!(h1, h2);
        }
    }

    #[test]
    fn test_commitment_hash_affects_final_hash() {
        let header = b"commitment test header";
        let height = 100u64;
        let nonce = 42u64;

        let seed = compute_epoch_seed(height);
        let mut dataset = generate_dataset(&seed);

        let hash1 = evo_omap_hash(&mut dataset, header, height, nonce);

        let seed2 = compute_epoch_seed(height + EPOCH_LENGTH);
        let mut dataset2 = generate_dataset(&seed2);
        let hash2 = evo_omap_hash(&mut dataset2, header, height, nonce);

        assert_ne!(
            hash1, hash2,
            "Different epoch seeds must produce different hashes"
        );

        let mut dataset3 = generate_dataset(&seed);
        let hash3 = evo_omap_hash(&mut dataset3, b"different header", height, nonce);

        assert_ne!(
            hash1, hash3,
            "Different headers must produce different hashes"
        );
    }

    #[test]
    fn test_dataset_generation_deterministic() {
        let seed = compute_epoch_seed(0);
        let ds1 = generate_dataset(&seed);
        let ds2 = generate_dataset(&seed);

        assert_eq!(ds1.nodes.len(), NUM_NODES);
        for i in 0..NUM_NODES {
            assert_eq!(ds1.nodes[i], ds2.nodes[i], "Node {} must be identical", i);
        }
    }

    // =============================================================================
    // 2. DATASET GENERATION TESTS
    // =============================================================================

    #[test]
    fn test_dataset_node_0_format() {
        let seed = compute_epoch_seed(0);
        let seed_bytes = seed.as_ref();

        let mut data = Vec::new();
        data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
        data.extend_from_slice(seed_bytes);
        data.extend_from_slice(&0u64.to_le_bytes());

        let expected_node_0 = blake3_xof(&data, NODE_SIZE);

        let ds = generate_dataset(&seed);
        assert_eq!(ds.nodes[0], expected_node_0);
    }

    #[test]
    fn test_dataset_node_i_chained() {
        let seed = compute_epoch_seed(0);
        let ds = generate_dataset(&seed);

        for i in 1..NUM_NODES {
            let seed_bytes = seed.as_ref();
            let mut data = Vec::new();
            data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
            data.extend_from_slice(seed_bytes);
            data.extend_from_slice(&ds.nodes[i - 1]);
            data.extend_from_slice(&(i as u64).to_le_bytes());

            let expected = blake3_xof(&data, NODE_SIZE);
            assert_eq!(
                ds.nodes[i],
                expected,
                "Node {} should be chained from node {}",
                i,
                i - 1
            );
        }
    }

    #[test]
    fn test_dataset_node_size_exactly_1mb() {
        let seed = compute_epoch_seed(0);
        let ds = generate_dataset(&seed);

        for i in 0..NUM_NODES {
            assert_eq!(ds.nodes[i].len(), NODE_SIZE, "Node {} should be 1 MiB", i);
            assert_eq!(
                ds.nodes[i].len(),
                1_048_576,
                "Node {} should be exactly 1,048,576 bytes",
                i
            );
        }
    }

    #[test]
    fn test_dataset_has_256_nodes() {
        let seed = compute_epoch_seed(0);
        let ds = generate_dataset(&seed);
        assert_eq!(ds.nodes.len(), 256);
    }

    #[test]
    fn test_different_seeds_different_datasets() {
        let seed0 = compute_epoch_seed(0);
        let seed1 = compute_epoch_seed(1024);
        let ds0 = generate_dataset(&seed0);
        let ds1 = generate_dataset(&seed1);

        assert_ne!(
            ds0.nodes[0], ds1.nodes[0],
            "Different seeds produce different node 0"
        );
        assert_ne!(
            ds0.nodes[128], ds1.nodes[128],
            "Different seeds produce different node 128"
        );
        assert_ne!(
            ds0.nodes[255], ds1.nodes[255],
            "Different seeds produce different node 255"
        );
    }

    // =============================================================================
    // 3. EPOCH SEED DERIVATION TESTS
    // =============================================================================

    #[test]
    fn test_epoch_seed_format() {
        let seed0 = compute_epoch_seed(0);

        let mut expected_data = Vec::new();
        expected_data.extend_from_slice(&prefixed_domain(DOMAIN_EPOCH));
        expected_data.extend_from_slice(&0u64.to_le_bytes());
        let expected = blake3_256(&expected_data);

        assert_eq!(seed0, expected);
    }

    #[test]
    fn test_epoch_seed_epoch_0_all_heights_same() {
        let seed0 = compute_epoch_seed(0);
        let seed1 = compute_epoch_seed(1);
        let seed512 = compute_epoch_seed(512);
        let seed1023 = compute_epoch_seed(1023);

        assert_eq!(seed0, seed1);
        assert_eq!(seed0, seed512);
        assert_eq!(seed0, seed1023);
    }

    #[test]
    fn test_epoch_seed_different_epochs_different_seeds() {
        let seed0 = compute_epoch_seed(0);
        let seed1 = compute_epoch_seed(1024);
        let seed2 = compute_epoch_seed(2048);

        assert_ne!(seed0, seed1);
        assert_ne!(seed1, seed2);
    }

    #[test]
    fn test_epoch_seed_is_32_bytes() {
        let seed = compute_epoch_seed(100);
        assert_eq!(seed.0.len(), 32);
    }

    // =============================================================================
    // 4. MINING SEED DERIVATION TESTS
    // =============================================================================

    #[test]
    fn test_mining_seed_format() {
        let header = b"test header";
        let height = 100u64;
        let nonce = 42u64;

        let header_commitment = blake3_256(header);
        let committed = header_commitment.as_ref();

        let mut expected_data = Vec::new();
        expected_data.extend_from_slice(&prefixed_domain(DOMAIN_SEED));
        expected_data.extend_from_slice(&(committed.len() as u64).to_le_bytes());
        expected_data.extend_from_slice(committed);
        expected_data.extend_from_slice(&height.to_le_bytes());
        expected_data.extend_from_slice(&nonce.to_le_bytes());

        let expected = blake3_256(&expected_data);
        let actual = compute_mining_seed(header, height, nonce);

        assert_eq!(expected, actual);
    }

    #[test]
    fn test_mining_seed_different_nonces_different_seeds() {
        let header = b"test header";
        let height = 100u64;

        let seed1 = compute_mining_seed(header, height, 0);
        let seed2 = compute_mining_seed(header, height, 1);

        assert_ne!(seed1, seed2);
    }

    #[test]
    fn test_mining_seed_different_headers_different_seeds() {
        let height = 100u64;
        let nonce = 42u64;

        let seed1 = compute_mining_seed(b"header1", height, nonce);
        let seed2 = compute_mining_seed(b"header2", height, nonce);

        assert_ne!(seed1, seed2);
    }

    // =============================================================================
    // 5. PROGRAM GENERATION TESTS
    // =============================================================================

    #[test]
    fn test_program_generation_always_8_instructions() {
        let seed = Hash([1u8; 32]);
        let state = State::from_seed(&seed);

        for _ in 0..100 {
            let rng_state = (state.as_u64_array()[0] ^ state.as_u64_array()[1]).wrapping_add(1);
            let test_state = State::from_seed(&Hash::from_bytes([(rng_state & 0xFF) as u8; 32]));
            let program = generate_program(&test_state);
            assert_eq!(program.instructions.len(), PROGRAM_LENGTH);
        }
    }

    #[test]
    fn test_program_generation_only_valid_opcodes() {
        let seed = Hash([42u8; 32]);
        let state = State::from_seed(&seed);
        let program = generate_program(&state);

        for instruction in &program.instructions {
            match instruction {
                Instruction::Add { .. } => {}
                Instruction::Sub { .. } => {}
                Instruction::Mul { .. } => {}
                Instruction::Xor { .. } => {}
                Instruction::Rotl { .. } => {}
                Instruction::Rotr { .. } => {}
                Instruction::Mulh { .. } => {}
                Instruction::Swap { .. } => {}
            }
        }
    }

    #[test]
    fn test_program_generation_src_register_always_valid() {
        let seed = Hash([99u8; 32]);
        let state = State::from_seed(&seed);
        let program = generate_program(&state);

        for instruction in &program.instructions {
            let src = match instruction {
                Instruction::Add { src, .. } => *src,
                Instruction::Sub { src, .. } => *src,
                Instruction::Mul { src, .. } => *src,
                Instruction::Xor { src, .. } => *src,
                Instruction::Rotl { .. } => continue,
                Instruction::Rotr { .. } => continue,
                Instruction::Mulh { src, .. } => *src,
                Instruction::Swap { .. } => continue,
            };
            assert!(src < 128, "src register {} must be 0-127", src);
        }
    }

    #[test]
    fn test_program_generation_dst_register_always_valid() {
        let seed = Hash([77u8; 32]);
        let state = State::from_seed(&seed);
        let program = generate_program(&state);

        for instruction in &program.instructions {
            let dst = match instruction {
                Instruction::Add { dst, .. } => *dst,
                Instruction::Sub { dst, .. } => *dst,
                Instruction::Mul { dst, .. } => *dst,
                Instruction::Xor { dst, .. } => *dst,
                Instruction::Rotl { dst, .. } => *dst,
                Instruction::Rotr { dst, .. } => *dst,
                Instruction::Mulh { dst, .. } => *dst,
                Instruction::Swap { a, .. } => *a,
            };
            assert!(dst < 8, "dst register {} must be 0-7", dst);
        }
    }

    #[test]
    fn test_program_generation_deterministic() {
        let seed = Hash([123u8; 32]);
        let state = State::from_seed(&seed);
        let p1 = generate_program(&state);
        let p2 = generate_program(&state);
        assert_eq!(p1.instructions, p2.instructions);
    }

    #[test]
    fn test_different_states_different_programs() {
        let state1 = State::from_seed(&Hash([1u8; 32]));
        let state2 = State::from_seed(&Hash([2u8; 32]));
        let p1 = generate_program(&state1);
        let p2 = generate_program(&state2);
        assert_ne!(p1.instructions, p2.instructions);
    }

    // =============================================================================
    // 6. INSTRUCTION EXECUTION TESTS
    // =============================================================================

    #[test]
    fn test_instruction_add_wrapping() {
        let mut state = [u64::MAX, 0, 0, 0, 0, 0, 0, 0];
        let node1_words = [1u64; 128];
        let node2_words = [0u64; 128];

        let add = Instruction::Add { dst: 0, src: 0 };
        add.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], 0);
    }

    #[test]
    fn test_instruction_sub_wrapping() {
        let mut state = [0u64, 0, 0, 0, 0, 0, 0, 0];
        let node1_words = [0u64; 128];
        let node2_words = [1u64; 128];

        let sub = Instruction::Sub { dst: 0, src: 0 };
        sub.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], u64::MAX);
    }

    #[test]
    fn test_instruction_mul_wrapping() {
        let mut state = [2u64, 0, 0, 0, 0, 0, 0, 0];
        let mut node1_words = [0u64; 128];
        node1_words[0] = u64::MAX;
        let node2_words = [0u64; 128];

        let mul = Instruction::Mul { dst: 0, src: 0 };
        mul.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], 2u64.wrapping_mul(u64::MAX));
    }

    #[test]
    fn test_instruction_xor() {
        let mut state = [0xFFu64, 0, 0, 0, 0, 0, 0, 0];
        let node1_words = [0u64; 128];
        let node2_words = [0x0Fu64; 128];

        let xor = Instruction::Xor { dst: 0, src: 0 };
        xor.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], 0xF0u64);
    }

    #[test]
    fn test_instruction_rotr_data_dependent() {
        let mut state = [0x01u64, 0, 0, 0, 0, 0, 0, 0];
        let mut node1_words = [0u64; 128];
        let node2_words = [0u64; 128];
        node1_words[0] = 1;

        let rotl = Instruction::Rotl { dst: 0, src: 0 };
        rotl.execute(&mut state, &node1_words, &node2_words);

        let val = 0x01u64;
        let expected = val.rotate_left((node1_words[0] % 64) as u32);
        assert_eq!(state[0], expected);
    }

    #[test]
    fn test_instruction_mulh_edge_case() {
        let mut state = [u64::MAX, 0, 0, 0, 0, 0, 0, 0];
        let node1_words = [u64::MAX; 128];
        let node2_words = [0u64; 128];

        let mulh = Instruction::Mulh { dst: 0, src: 0 };
        mulh.execute(&mut state, &node1_words, &node2_words);

        let wide: u128 = (u64::MAX as u128).wrapping_mul(u64::MAX as u128);
        let expected = (wide >> 64) as u64;
        assert_eq!(state[0], expected);
    }

    #[test]
    fn test_instruction_swap() {
        let mut state = [1u64, 2, 3, 4, 5, 6, 7, 8];
        let node1_words = [0u64; 128];
        let node2_words = [0u64; 128];

        let swap = Instruction::Swap { a: 0, b: 1 };
        swap.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], 2);
        assert_eq!(state[1], 1);
    }

    // =============================================================================
    // 7. BRANCH OPERATION TESTS
    // =============================================================================

    #[test]
    fn test_branch_variant_range() {
        for _ in 0..100 {
            let state = State::from_seed(&Hash([rand::random(); 32]));
            let words = state.as_u64_array();
            let variant = words[0] & 0x03;
            assert!(variant <= 3, "Branch variant must be 0-3");
        }
    }

    #[test]
    fn test_branch_xor_not_replace() {
        let seed1 = Hash([1u8; 32]);
        let seed2 = Hash([2u8; 32]);

        let state1_orig = State::from_seed(&seed1);
        let state2_orig = State::from_seed(&seed2);

        let node1 = vec![0xFFu8; 1024];
        let node2 = vec![0xAAu8; 1024];

        let mut s1 = state1_orig.clone();
        let mut s2 = state2_orig.clone();

        apply_branch(&mut s1, 0, &node1, &node2);
        apply_branch(&mut s2, 0, &node1, &node2);

        let orig_words = state1_orig.as_u64_array();
        let new_words = s1.as_u64_array();
        let mut any_different = false;
        for i in 0..8 {
            if orig_words[i] != new_words[i] {
                any_different = true;
                break;
            }
        }
        assert!(any_different, "Branch must modify state via XOR");

        assert_ne!(
            s1.as_u64_array(),
            s2.as_u64_array(),
            "Different states must produce different branches"
        );
    }

    #[test]
    fn test_branch_distribution_uniform() {
        let mut counts = [0u64; 4];

        for i in 0..10000 {
            let seed = Hash::from_bytes({
                let mut arr = [0u8; 32];
                arr[0] = (i & 0xFF) as u8;
                arr
            });
            let state = State::from_seed(&seed);
            let words = state.as_u64_array();
            let variant = (words[0] & 0x03) as usize;
            counts[variant] += 1;
        }

        for i in 0..4 {
            let percentage = counts[i] as f64 / 10000.0;
            assert!(
                percentage > 0.15 && percentage < 0.40,
                "Branch {} has {}%, expected 20-35%",
                i,
                percentage * 100.0
            );
        }
    }

    // =============================================================================
    // 8. WRITE STEP TESTS
    // =============================================================================

    #[test]
    fn test_write_step_mutates_dataset() {
        let header = b"test write";
        let height = 50u64;
        let nonce = 0u64;

        let seed = compute_epoch_seed(height);
        let base_ds = generate_dataset(&seed);

        let mut ds = Dataset::new();
        for i in 0..NUM_NODES {
            ds.set(i, base_ds.get(i).to_vec());
        }

        let original_node_100 = ds.get(100).to_vec();

        let _hash = evo_omap_hash(&mut ds, header, height, nonce);

        let final_node_100 = ds.get(100).to_vec();
        assert_ne!(
            original_node_100, final_node_100,
            "Dataset node should be modified"
        );
    }

    #[test]
    fn test_write_step_deterministic() {
        let header = b"deterministic write test";
        let height = 75u64;
        let nonce = 123u64;

        let seed = compute_epoch_seed(height);
        let base_ds = generate_dataset(&seed);

        let mut ds1 = Dataset::new();
        let mut ds2 = Dataset::new();
        for i in 0..NUM_NODES {
            ds1.set(i, base_ds.get(i).to_vec());
            ds2.set(i, base_ds.get(i).to_vec());
        }

        let _hash1 = evo_omap_hash(&mut ds1, header, height, nonce);
        let _hash2 = evo_omap_hash(&mut ds2, header, height, nonce);

        for i in 0..NUM_NODES {
            assert_eq!(
                ds1.nodes[i], ds2.nodes[i],
                "Same inputs must produce same write at node {}",
                i
            );
        }
    }

    // =============================================================================
    // 9. ROLLING COMMITMENT TESTS
    // =============================================================================

    #[test]
    fn test_rolling_commitment_deterministic() {
        let header = b"rolling commitment test";
        let height = 200u64;
        let nonce = 0u64;

        let seed = compute_epoch_seed(height);
        let mut ds1 = generate_dataset(&seed);
        let mut ds2 = generate_dataset(&seed);

        let _hash1 = evo_omap_hash(&mut ds1, header, height, nonce);
        let _hash2 = evo_omap_hash(&mut ds2, header, height, nonce);
    }

    #[test]
    fn test_rolling_commitment_different_paths_different_final() {
        let header = b"rolling path test";
        let height = 300u64;

        let seed = compute_epoch_seed(height);

        let mut ds1 = generate_dataset(&seed);
        let h1 = evo_omap_hash(&mut ds1, header, height, 0);

        let mut ds2 = generate_dataset(&seed);
        let h2 = evo_omap_hash(&mut ds2, header, height, 1);

        assert_ne!(
            h1, h2,
            "Different nonces must produce different hashes and thus different final commitments"
        );
    }

    // =============================================================================
    // 10. MEMORY COMMITMENT TESTS
    // =============================================================================

    #[test]
    fn test_memory_commitment_format() {
        let node_0 = b"node zero".as_slice();
        let node_1 = b"node one".as_slice();
        let node_2 = b"node two".as_slice();
        let nodes = [node_0, node_1, node_2];

        let leaf_0 = compute_memory_leaf_hash(0, node_0);
        let leaf_1 = compute_memory_leaf_hash(1, node_1);
        let leaf_2 = compute_memory_leaf_hash(2, node_2);
        let parent_0 = compute_memory_parent_hash(&leaf_0, &leaf_1);
        let parent_1 = compute_memory_parent_hash(&leaf_2, &leaf_2);
        let expected = compute_memory_parent_hash(&parent_0, &parent_1);

        let computed = compute_memory_commitment_from_slice(&nodes);

        assert_eq!(computed, expected);
    }

    #[test]
    fn test_memory_merkle_proof_verifies_node_membership() {
        let nodes: [&[u8]; 4] = [b"node 0", b"node 1", b"node 2", b"node 3"];
        let root = compute_memory_commitment_from_slice(&nodes);
        let proof = build_memory_merkle_proof_from_slice(&nodes, 2).unwrap();

        assert!(verify_memory_merkle_proof(&root, nodes[2], &proof));
        assert!(!verify_memory_merkle_proof(&root, b"tampered", &proof));
    }

    #[test]
    fn test_memory_merkle_proof_rejects_out_of_range_index() {
        let nodes: [&[u8]; 2] = [b"node 0", b"node 1"];
        assert!(build_memory_merkle_proof_from_slice(&nodes, 2).is_none());
    }

    #[test]
    fn test_memory_commitment_changes_with_dataset() {
        let seed = compute_epoch_seed(0);
        let ds = generate_dataset(&seed);
        let root1 = compute_memory_commitment(&ds);

        let mut modified_ds = generate_dataset(&seed);
        modified_ds.nodes[0][0] ^= 0x01;
        let root2 = compute_memory_commitment(&modified_ds);

        assert_ne!(
            root1, root2,
            "Changing any node must change memory commitment"
        );
    }

    // =============================================================================
    // 11. FINAL HASH TESTS
    // =============================================================================

    #[test]
    fn test_final_hash_uses_sha3_256() {
        let header = b"final hash test";
        let height = 500u64;
        let nonce = 0u64;

        let seed = compute_epoch_seed(height);
        let mut ds = generate_dataset(&seed);
        let hash = evo_omap_hash(&mut ds, header, height, nonce);

        assert_eq!(hash.0.len(), 32);
    }

    #[test]
    fn test_final_hash_different_inputs_different_outputs() {
        let seed = compute_epoch_seed(0);

        let mut ds1 = generate_dataset(&seed);
        let h1 = evo_omap_hash(&mut ds1, b"header1", 0, 0);

        let mut ds2 = generate_dataset(&seed);
        let h2 = evo_omap_hash(&mut ds2, b"header2", 0, 0);

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_final_hash_deterministic() {
        let header = b"deterministic final hash";
        let height = 600u64;
        let nonce = 99u64;

        let seed = compute_epoch_seed(height);
        let mut ds1 = generate_dataset(&seed);
        let mut ds2 = generate_dataset(&seed);

        let h1 = evo_omap_hash(&mut ds1, header, height, nonce);
        let h2 = evo_omap_hash(&mut ds2, header, height, nonce);

        assert_eq!(h1, h2);
    }

    // =============================================================================
    // 12. LIGHT VS FULL VERIFICATION TESTS
    // =============================================================================

    #[test]
    fn test_light_and_full_produce_same_hash() {
        let header = b"light vs full verification";
        let height = 50u64;
        let nonce = 0u64;

        let seed = compute_epoch_seed(height);

        let mut ds_full = generate_dataset(&seed);
        let hash_full = evo_omap_hash(&mut ds_full, header, height, nonce);

        let mut ds_light = Dataset::new();
        let mut prev_node = Vec::new();
        for i in 0..NUM_NODES {
            let node = if i == 0 {
                let mut data = Vec::new();
                data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
                data.extend_from_slice(seed.as_ref());
                data.extend_from_slice(&(0u64).to_le_bytes());
                blake3_xof(&data, NODE_SIZE)
            } else {
                let mut data = Vec::new();
                data.extend_from_slice(&prefixed_domain(DOMAIN_NODE));
                data.extend_from_slice(seed.as_ref());
                data.extend_from_slice(&prev_node);
                data.extend_from_slice(&(i as u64).to_le_bytes());
                blake3_xof(&data, NODE_SIZE)
            };
            ds_light.set(i, node.clone());
            prev_node = node;
        }
        let hash_light = evo_omap_hash(&mut ds_light, header, height, nonce);

        assert_eq!(hash_full, hash_light);
    }

    // =============================================================================
    // 13. DOMAIN SEPARATOR UNIQUENESS TESTS
    // =============================================================================

    #[test]
    fn test_domain_separators_are_distinct() {
        let data = b"test data for domain separator";

        let hash_epoch = blake3_256(
            &[DOMAIN_EPOCH, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let hash_node = blake3_256(
            &[DOMAIN_NODE, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let hash_seed = blake3_256(
            &[DOMAIN_SEED, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let hash_cache = blake3_256(
            &[DOMAIN_CACHE, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let hash_branch = blake3_256(
            &[DOMAIN_BRANCH, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let hash_commitment = blake3_256(
            &[DOMAIN_COMMITMENT, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let hash_memory = blake3_256(
            &[DOMAIN_MEMORY, data]
                .iter()
                .flat_map(|v| v.iter())
                .cloned()
                .collect::<Vec<_>>(),
        );

        let hashes = [
            hash_epoch,
            hash_node,
            hash_seed,
            hash_cache,
            hash_branch,
            hash_commitment,
            hash_memory,
        ];

        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(
                    hashes[i], hashes[j],
                    "Domain separator {} must be distinct from {}",
                    i, j
                );
            }
        }
    }

    // =============================================================================
    // 14. OPERAND BOUNDS TESTS
    // =============================================================================

    #[test]
    fn test_operand_access_never_exceeds_127() {
        let header = b"operand bounds test";
        let height = 700u64;

        let seed = compute_epoch_seed(height);
        let mut ds = generate_dataset(&seed);

        let _hash = evo_omap_hash(&mut ds, header, height, 0);

        for i in 0..NUM_NODES {
            assert_eq!(ds.nodes[i].len(), NODE_SIZE);
        }
    }

    #[test]
    fn test_derive_indices_always_in_range() {
        for step in 0..100u64 {
            let state = State::from_seed(&Hash::from_bytes({
                let mut arr = [0u8; 32];
                arr[0..8].copy_from_slice(&step.to_le_bytes());
                arr
            }));

            let (idx1, idx2, idx_write) = derive_indices(&state, step);

            assert!(idx1 < NUM_NODES, "idx1 must be < {}", NUM_NODES);
            assert!(idx2 < NUM_NODES, "idx2 must be < {}", NUM_NODES);
            assert!(idx_write < NUM_NODES, "idx_write must be < {}", NUM_NODES);
        }
    }

    // =============================================================================
    // 15. COPY-ON-WRITE MINING TESTS
    // =============================================================================

    #[test]
    fn test_cow_produces_same_result_as_naive() {
        let header = b"CoW mining test";
        let height = 800u64;
        let seed = compute_epoch_seed(height);

        let base_ds = generate_dataset(&seed);

        for nonce in 0..5u64 {
            let mut ds_naive = Dataset::new();
            for i in 0..NUM_NODES {
                ds_naive.set(i, base_ds.get(i).to_vec());
            }
            let hash_naive = evo_omap_hash(&mut ds_naive, header, height, nonce);

            let mut cow = CowDataset::new(&base_ds);
            cow.reset();
            let mut ds_cow = Dataset::new();
            for i in 0..NUM_NODES {
                ds_cow.set(i, cow.get(i).to_vec());
            }
            let hash_cow = evo_omap_hash(&mut ds_cow, header, height, nonce);

            assert_eq!(
                hash_naive, hash_cow,
                "CoW must produce same result as naive for nonce {}",
                nonce
            );
        }
    }

    // =============================================================================
    // 16. EPOCH BOUNDARY TESTS
    // =============================================================================

    #[test]
    fn test_epoch_boundary_different_datasets() {
        let seed0 = compute_epoch_seed(0);
        let seed1023 = compute_epoch_seed(1023);
        let seed1024 = compute_epoch_seed(1024);

        assert_eq!(seed0, seed1023);
        assert_ne!(seed0, seed1024);

        let ds0 = generate_dataset(&seed0);
        let ds1024 = generate_dataset(&seed1024);

        assert_ne!(ds0.nodes[0], ds1024.nodes[0]);
        assert_ne!(ds0.nodes[128], ds1024.nodes[128]);
        assert_ne!(ds0.nodes[255], ds1024.nodes[255]);
    }

    // =============================================================================
    // 17. DIFFICULTY VALIDATION TESTS
    // =============================================================================

    #[test]
    fn test_difficulty_validation_easy() {
        let header = b"easy difficulty test";
        let height = 900u64;
        let difficulty = 1u64;
        let target = u64::MAX / difficulty;

        let seed = compute_epoch_seed(height);
        let mut ds = generate_dataset(&seed);
        let hash = evo_omap_hash(&mut ds, header, height, 0);
        let hash_int = u64::from_be_bytes(hash.0[..8].try_into().unwrap());

        assert!(hash_int < target, "Difficulty 1 should accept most hashes");
    }

    #[test]
    fn test_difficulty_validation_hard() {
        let header = b"hard difficulty test";
        let height = 950u64;
        let difficulty = 1_000_000u64;

        assert!(!verify(header, height, 0, difficulty));
    }

    #[test]
    fn test_verify_accepts_valid_proof() {
        let header = b"valid proof test";
        let height = 1000u64;
        let difficulty = 1u64;

        let seed = compute_epoch_seed(height);
        let base_ds = generate_dataset(&seed);

        let mut found_nonce = None;
        for nonce in 0..1_000_000u64 {
            let mut ds = Dataset::new();
            for i in 0..NUM_NODES {
                ds.set(i, base_ds.get(i).to_vec());
            }
            let hash = evo_omap_hash(&mut ds, header, height, nonce);
            let leading_zeros = hash
                .0
                .iter()
                .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
                .take_while(|&b| b == 0)
                .count() as u64;
            if leading_zeros >= difficulty {
                found_nonce = Some(nonce);
                break;
            }
        }

        assert!(
            found_nonce.is_some(),
            "Failed to find valid nonce in 1M attempts"
        );
        let nonce = found_nonce.unwrap();
        assert!(verify(header, height, nonce, difficulty));
    }

    // =============================================================================
    // 18. EDGE CASES TESTS
    // =============================================================================

    #[test]
    fn test_edge_case_zero_seed() {
        let seed = Hash([0u8; 32]);
        let state = State::from_seed(&seed);
        assert_ne!(state.as_u64_array(), [0u64; 8]);
    }

    #[test]
    fn test_edge_case_max_seed() {
        let seed = Hash([0xFFu8; 32]);
        let state = State::from_seed(&seed);
        assert_ne!(state.as_u64_array(), [0u64; 8]);
    }

    #[test]
    fn test_edge_case_empty_header() {
        let header: &[u8] = &[];
        let height = 0u64;
        let nonce = 0u64;

        let seed = compute_epoch_seed(height);
        let mut ds = generate_dataset(&seed);
        let hash = evo_omap_hash(&mut ds, header, height, nonce);

        assert_eq!(hash.0.len(), 32);
    }

    #[test]
    fn test_edge_case_nonce_max() {
        let header = b"max nonce test";
        let height = 0u64;
        let nonce = u64::MAX;

        let seed = compute_epoch_seed(height);
        let mut ds1 = generate_dataset(&seed);
        let mut ds2 = generate_dataset(&seed);

        let h1 = evo_omap_hash(&mut ds1, header, height, nonce);
        let h2 = evo_omap_hash(&mut ds2, header, height, nonce);

        assert_eq!(h1, h2);
    }

    #[test]
    fn test_edge_case_difficulty_1() {
        let header = b"difficulty 1 test";
        let height = 1100u64;
        let difficulty = 1u64;

        let seed = compute_epoch_seed(height);
        let mut ds = generate_dataset(&seed);
        let hash = evo_omap_hash(&mut ds, header, height, 0);
        let hash_int = u64::from_be_bytes(hash.0[..8].try_into().unwrap());
        let target = u64::MAX / difficulty;

        assert!(hash_int < target);
    }

    #[test]
    fn test_edge_case_integer_overflow_mul() {
        let mut state = [u64::MAX, 0, 0, 0, 0, 0, 0, 0];
        let node1_words = [u64::MAX; 128];
        let node2_words = [0u64; 128];

        let mul = Instruction::Mul { dst: 0, src: 0 };
        mul.execute(&mut state, &node1_words, &node2_words);

        let expected = u64::MAX.wrapping_mul(u64::MAX);
        assert_eq!(state[0], expected);
    }

    #[test]
    fn test_edge_case_mulh_max_values() {
        let mut state = [u64::MAX, 0, 0, 0, 0, 0, 0, 0];
        let node1_words = [u64::MAX; 128];
        let node2_words = [0u64; 128];

        let mulh = Instruction::Mulh { dst: 0, src: 0 };
        mulh.execute(&mut state, &node1_words, &node2_words);

        let wide: u128 = (u64::MAX as u128).wrapping_mul(u64::MAX as u128);
        let expected = (wide >> 64) as u64;
        assert_eq!(state[0], expected);
    }

    #[test]
    fn test_edge_case_rotr_by_0() {
        let mut state = [0x123456789ABCDEFu64, 0, 0, 0, 0, 0, 0, 0];
        let mut node1_words = [0u64; 128];
        let node2_words = [0u64; 128];
        node1_words[0] = 0;

        let rotr = Instruction::Rotr { dst: 0, src: 0 };
        rotr.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], 0x123456789ABCDEF);
    }

    #[test]
    fn test_edge_case_rotl_by_63() {
        let mut state = [1u64, 0, 0, 0, 0, 0, 0, 0];
        let mut node1_words = [0u64; 128];
        let node2_words = [0u64; 128];
        node1_words[0] = 63;

        let rotl = Instruction::Rotl { dst: 0, src: 0 };
        rotl.execute(&mut state, &node1_words, &node2_words);

        let val = 1u64;
        let expected = val.rotate_left((node1_words[0] % 64) as u32);
        assert_eq!(state[0], expected);
    }

    // =============================================================================
    // 19. KNOWN-ANSWER TESTS
    // =============================================================================

    #[test]
    fn test_known_answer_epoch_seed() {
        let seed = compute_epoch_seed(0);
        // KAT: verify epoch seed 0 matches the prefixed-domain format.
        let mut expected_data = Vec::new();
        expected_data.extend_from_slice(&prefixed_domain(DOMAIN_EPOCH));
        expected_data.extend_from_slice(&0u64.to_le_bytes());
        let expected = blake3_256(&expected_data);
        assert_eq!(
            seed, expected,
            "Epoch seed 0 must match prefixed-domain format"
        );
        assert_ne!(seed.0, [0u8; 32], "Epoch seed must not be zero");
    }

    #[test]
    fn test_known_answer_deterministic_hash() {
        let header = b"known answer test";
        let height = 1024u64;
        let nonce = 42u64;

        let seed = compute_epoch_seed(height);
        let mut ds = generate_dataset(&seed);
        let hash = evo_omap_hash(&mut ds, header, height, nonce);

        let hash_int = u64::from_be_bytes(hash.0[..8].try_into().unwrap());
        assert!(hash_int != 0, "Hash should not be zero");

        assert_eq!(hash.0.len(), 32, "Hash should be 32 bytes");
    }

    #[test]
    fn test_known_answer_verify_rejects_wrong_nonce() {
        let header = b"wrong nonce test";
        let height = 2000u64;
        let difficulty = 1000u64;

        assert!(!verify(header, height, 999999, difficulty));
    }

    // =============================================================================
    // STATE CONVERSION TESTS
    // =============================================================================

    #[test]
    fn test_state_as_u64_array() {
        let state = State([
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15,
            0x16, 0x17, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x30, 0x31, 0x32, 0x33,
            0x34, 0x35, 0x36, 0x37, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x50, 0x51,
            0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67,
            0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77,
        ]);
        let arr = state.as_u64_array();
        assert_eq!(arr[0], 0x0706050403020100u64);
        assert_eq!(arr[1], 0x1716151413121110u64);
        assert_eq!(arr[7], 0x7776757473727170u64);
    }

    #[test]
    fn test_state_from_seed() {
        let seed = Hash([0u8; 32]);
        let state = State::from_seed(&seed);
        let arr = state.as_u64_array();
        assert_ne!(arr, [0u64; 8]);
    }

    // =============================================================================
    // INDEX DERIVATION TESTS
    // =============================================================================

    #[test]
    fn test_derive_indices() {
        let state = State([0u8; 64]);
        let (i1, i2, iw) = derive_indices(&state, 0);
        assert_eq!(i1, 0);
        assert_eq!(i2, 0);
        assert_eq!(iw, 0);
    }

    // =============================================================================
    // DATASET MODIFICATION TESTS
    // =============================================================================

    #[test]
    fn test_dataset_modification_after_hash() {
        let header = b"dataset modification test";
        let height = 3000u64;
        let nonce = 0u64;

        let seed = compute_epoch_seed(height);
        let base_ds = generate_dataset(&seed);

        let mut ds = Dataset::new();
        for i in 0..NUM_NODES {
            ds.set(i, base_ds.get(i).to_vec());
        }

        let original_node_0 = ds.get(0).to_vec();

        let _hash = evo_omap_hash(&mut ds, header, height, nonce);

        let final_node_0 = ds.get(0).to_vec();
        assert_ne!(
            original_node_0, final_node_0,
            "Dataset node should be modified after hash"
        );
    }

    #[test]
    fn test_chained_dataset_generation() {
        let seed = compute_epoch_seed(0);
        let ds = generate_dataset(&seed);
        assert_ne!(ds.nodes[0], ds.nodes[1]);
        assert_ne!(ds.nodes[1], ds.nodes[2]);
        assert_ne!(ds.nodes[254], ds.nodes[255]);
    }

    #[test]
    fn test_node_size() {
        let seed = compute_epoch_seed(0);
        let ds = generate_dataset(&seed);
        assert_eq!(ds.nodes[0].len(), NODE_SIZE);
        assert_eq!(ds.nodes[255].len(), NODE_SIZE);
    }

    // =============================================================================
    // ADDITIONAL COMPREHENSIVE TESTS
    // =============================================================================

    #[test]
    fn test_full_hash_deterministic() {
        let header = b"test header for evo-omap";
        let height = 100u64;
        let nonce = 42u64;

        let seed0 = compute_epoch_seed(height);
        let mut ds1 = generate_dataset(&seed0);
        let mut ds2 = generate_dataset(&seed0);
        let h1 = evo_omap_hash(&mut ds1, header, height, nonce);
        let h2 = evo_omap_hash(&mut ds2, header, height, nonce);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_different_nonce_different_hash() {
        let header = b"test header";
        let height = 100u64;

        let seed = compute_epoch_seed(height);
        let mut ds1 = generate_dataset(&seed);
        let mut ds2 = generate_dataset(&seed);
        let h1 = evo_omap_hash(&mut ds1, header, height, 0);
        let h2 = evo_omap_hash(&mut ds2, header, height, 1);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_different_height_different_hash() {
        let header = b"test header";
        let nonce = 0u64;

        let seed0 = compute_epoch_seed(0);
        let seed1 = compute_epoch_seed(1);
        let mut ds1 = generate_dataset(&seed0);
        let mut ds2 = generate_dataset(&seed1);
        let h1 = evo_omap_hash(&mut ds1, header, 0, nonce);
        let h2 = evo_omap_hash(&mut ds2, header, 1, nonce);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_verify_rejects_invalid_proof() {
        let header = b"test header";
        let height = 100u64;
        let difficulty = 1_000_000u64;
        assert!(!verify(header, height, 0, difficulty));
    }

    #[test]
    fn test_light_vs_full_verification() {
        let header = b"test header for light verification";
        let height = 50u64;
        let difficulty = 1u64;

        let seed = compute_epoch_seed(height);
        let base_ds = generate_dataset(&seed);

        let mut found_nonce = None;
        for nonce in 0..500_000u64 {
            let mut ds = Dataset::new();
            for i in 0..NUM_NODES {
                ds.set(i, base_ds.get(i).to_vec());
            }
            let hash = evo_omap_hash(&mut ds, header, height, nonce);
            let leading_zeros = hash
                .0
                .iter()
                .flat_map(|b| (0..8u32).rev().map(move |i| (b >> i) & 1))
                .take_while(|&b| b == 0)
                .count() as u64;
            if leading_zeros >= difficulty {
                found_nonce = Some(nonce);
                break;
            }
        }

        if let Some(nonce) = found_nonce {
            assert!(verify(header, height, nonce, difficulty));
            assert!(verify_light(header, height, nonce, difficulty));
        }
    }

    #[test]
    fn test_epoch_boundary() {
        let seed0 = compute_epoch_seed(0);
        let seed1023 = compute_epoch_seed(1023);
        let seed1024 = compute_epoch_seed(1024);
        assert_eq!(seed0, seed1023);
        assert_ne!(seed0, seed1024);
    }

    #[test]
    fn test_generate_dataset_deterministic() {
        let seed = compute_epoch_seed(0);
        let ds1 = generate_dataset(&seed);
        let ds2 = generate_dataset(&seed);
        assert_eq!(ds1.nodes.len(), NUM_NODES);
        assert_eq!(ds1.nodes.len(), ds2.nodes.len());
        for i in 0..NUM_NODES {
            assert_eq!(ds1.nodes[i], ds2.nodes[i]);
        }
    }

    #[test]
    fn test_generate_program_deterministic() {
        let seed = Hash([1u8; 32]);
        let state = State::from_seed(&seed);
        let p1 = generate_program(&state);
        let p2 = generate_program(&state);
        assert_eq!(p1.instructions, p2.instructions);
    }

    #[test]
    fn test_light_dataset_reset() {
        let seed = compute_epoch_seed(0);
        let mut light_ds = LightDataset::new(&seed);
        let original = light_ds.get_node(0);
        let dummy_node = vec![0xABu8; NODE_SIZE];
        light_ds.set_node(0, dummy_node.clone());
        assert_eq!(light_ds.get_node(0), dummy_node);
        light_ds.reset();
        let after_reset = light_ds.get_node(0);
        assert_ne!(after_reset, dummy_node, "reset() must clear modified nodes");
        assert_eq!(after_reset, original, "reset() must restore original node");
        assert_eq!(after_reset.len(), NODE_SIZE);
    }

    #[test]
    fn test_compute_epoch_seed() {
        let seed0 = compute_epoch_seed(0);
        let seed1 = compute_epoch_seed(1);
        let seed1023 = compute_epoch_seed(1023);
        let seed1024 = compute_epoch_seed(1024);
        let seed1025 = compute_epoch_seed(1025);
        assert_eq!(seed0, compute_epoch_seed(0));
        assert_eq!(seed0, seed1);
        assert_eq!(seed0, seed1023);
        assert_ne!(seed0, seed1024);
        assert_eq!(seed1024, seed1025);
    }
}
