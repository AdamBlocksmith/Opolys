//! # EVO-OMAP Public Specification
//!
//! This module contains the **public algorithm specification** for EVO-OMAP,
//! an execution-based, memory-hard proof-of-work algorithm.
//!
//! ## Public vs Private
//!
//! ### What is PUBLIC (in this file):
//! - Algorithm design and specification
//! - Instruction set and operation semantics
//! - Parameter valid ranges
//! - Step execution order and data flow
//! - Security properties and rationale
//! - Domain separators
//!
//! ### What is PRIVATE (NOT in this file):
//! - Exact memory size in bytes
//! - Exact compute_steps count
//! - Branching configuration values
//! - Cache size and structure
//! - Epoch length
//! - Network-specific tuning parameters
//!
//! ## Architecture Overview
//!
//! EVO-OMAP is a proof-of-work algorithm that:
//!
//! 1. **Generates a dataset** from an epoch seed using chained Blake3 XOF
//! 2. **Initializes state** from a mining seed (header + height + nonce)
//! 3. **Executes N steps** where each step:
//!    - Derives dataset indices from current state
//!    - Reads dataset nodes
//!    - Executes a program of instructions
//!    - Applies data-dependent branching
//!    - Writes back to dataset
//!    - Updates rolling commitment
//! 4. **Finalizes** by hashing state summary with dataset commitment
//!
//! ## Security Design
//!
//! EVO-OMAP achieves ASIC resistance through:
//!
//! - **Memory hardness**: Large, mutable dataset requires R/W memory
//! - **Data-dependent branching**: Unpredictable execution paths prevent fixed pipelines
//! - **Complex instruction set**: Rotations, multiplies, area-expensive operations
//! - **Sequential dataset generation**: Cannot precompute nodes in parallel
//! - **State entanglement**: Each step's branch depends on all prior execution

/// State size in bytes.
///
/// The state is a 64-byte vector interpreted as 8 × 64-bit words.
/// All state operations interpret bytes as little-endian u64 values.
/// This is explicitly little-endian for cross-platform consistency.
pub const STATE_SIZE: usize = 64;

/// Number of state registers (8 × 64-bit = 512 bits).
pub const NUM_REGISTERS: usize = 8;

/// Number of operand u64 words per node that instructions can access.
///
/// Instructions access node_word[src % OPERAND_WORDS], limiting operand
/// reads to the first 128 × 8 = 1024 bytes (1 KiB) of each 1 MiB node.
/// This is a cache-friendly design choice for light verification.
pub const OPERAND_WORDS: usize = 128;

/// Minimum program length (instructions per step).
pub const PROGRAM_LENGTH_MIN: usize = 4;

/// Maximum program length (instructions per step).
pub const PROGRAM_LENGTH_MAX: usize = 16;

/// Minimum branch ways (2 = binary, 4 = quad, 8 = octal).
pub const BRANCH_WAYS_MIN: u8 = 2;

/// Maximum branch ways.
pub const BRANCH_WAYS_MAX: u8 = 8;

/// Minimum compute steps per hash.
pub const STEPS_MIN: usize = 512;

/// Maximum compute steps per hash.
pub const STEPS_MAX: usize = 65536;

/// Minimum epoch length in blocks.
pub const EPOCH_LENGTH_MIN: u64 = 128;

/// Maximum epoch length in blocks.
pub const EPOCH_LENGTH_MAX: u64 = 8192;

// =============================================================================
// Instruction Set
// =============================================================================

/// Instruction set for EVO-OMAP superscalar execution.
///
/// Each instruction operates on the 8-word state array and reads
/// from dataset nodes. The instruction set is designed to be:
/// - CPU-friendly (common operations)
/// - ASIC-resistant (some operations are area-expensive)
/// - Data-dependent (branching on intermediate values)
///
/// ## Operations
///
/// | Op | Name | Description | ASIC Impact |
/// |----|------|-------------|--------------|
/// | ADD | Add | state[dst] = state[dst] + node_word[src % 128] | Low |
/// | SUB | Subtract | state[dst] = state[dst] - node_word[src % 128] | Low |
/// | MUL | Multiply | state[dst] = state[dst] * node_word[src % 128] | Medium |
/// | XOR | XOR | state[dst] = state[dst] ^ node_word[src % 128] | Low |
/// | ROTL | Rotate Left | state[dst].rotate_left(amount) | High (barrel shifter) |
/// | ROTR | Rotate Right | state[dst].rotate_right(amount) | High (barrel shifter) |
/// | MULH | Multiply High | High 64 bits of state[dst] * operand | Very High (128-bit mult) |
/// | SWAP | Swap | Exchange state[a] and state[b] | Low |
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Instruction {
    Add { dst: u8, src: u8 },
    Sub { dst: u8, src: u8 },
    Mul { dst: u8, src: u8 },
    Xor { dst: u8, src: u8 },
    Rotl { dst: u8, src: u8 },
    Rotr { dst: u8, src: u8 },
    Mulh { dst: u8, src: u8 },
    Swap { a: u8, b: u8 },
}

impl Instruction {
    /// Executes this instruction on the given state.
    ///
    /// # Arguments
    /// * `state` - Array of 8 u64 words representing execution state
    /// * `node1_words` - Words from first dataset node (for Add, Mul, Mulh)
    /// * `node2_words` - Words from second dataset node (for Sub, Xor)
    ///
    /// # Notes
    /// - All arithmetic uses wrapping to prevent overflow attacks
    /// - Rotation amounts are data-dependent: node_word[src % OPERAND_WORDS] % 64
    /// - All arithmetic/logic instructions access node data for memory-hardness
    pub fn execute(&self, state: &mut [u64], node1_words: &[u64], node2_words: &[u64]) {
        match self {
            Instruction::Add { dst, src } => {
                let operand = node1_words[(*src as usize) % OPERAND_WORDS];
                state[*dst as usize] = state[*dst as usize].wrapping_add(operand);
            }
            Instruction::Sub { dst, src } => {
                let operand = node2_words[(*src as usize) % OPERAND_WORDS];
                state[*dst as usize] = state[*dst as usize].wrapping_sub(operand);
            }
            Instruction::Mul { dst, src } => {
                let operand = node1_words[(*src as usize) % OPERAND_WORDS];
                state[*dst as usize] = state[*dst as usize].wrapping_mul(operand);
            }
            Instruction::Xor { dst, src } => {
                let operand = node2_words[(*src as usize) % OPERAND_WORDS];
                state[*dst as usize] ^= operand;
            }
            Instruction::Rotl { dst, src } => {
                let amount = node1_words[(*src as usize) % OPERAND_WORDS] % 64;
                state[*dst as usize] = state[*dst as usize].rotate_left(amount as u32);
            }
            Instruction::Rotr { dst, src } => {
                let amount = node2_words[(*src as usize) % OPERAND_WORDS] % 64;
                state[*dst as usize] = state[*dst as usize].rotate_right(amount as u32);
            }
            Instruction::Mulh { dst, src } => {
                let operand = node1_words[(*src as usize) % OPERAND_WORDS] as u128;
                let wide = (state[*dst as usize] as u128).wrapping_mul(operand);
                state[*dst as usize] = (wide >> 64) as u64;
            }
            Instruction::Swap { a, b } => {
                state.swap(
                    (*a % NUM_REGISTERS as u8) as usize,
                    (*b % NUM_REGISTERS as u8) as usize,
                );
            }
        }
    }
}

/// A superscalar program is a variable-length sequence of instructions
/// that execute in order during each step.
///
/// The program length is a tuning parameter between PROGRAM_LENGTH_MIN
/// and PROGRAM_LENGTH_MAX.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    /// The instructions in this program.
    pub instructions: Vec<Instruction>,
}

/// Program generation bit manipulation constants.
///
/// These define how instructions are selected from state bits:
/// - 3 bits for operation selection (8 possible operations)
/// - 3 bits for destination register (0-7)
/// - 7 bits for source register index (0-127), masked with SRC_MASK
pub const PROGRAM_GEN_OP_BITS: usize = 3;
pub const PROGRAM_GEN_REG_BITS: usize = 3;

// =============================================================================
// Branching
// =============================================================================

pub fn select_branch_variant(state_words: &[u64; 8], branch_ways: u8) -> u8 {
    (state_words[0] & ((branch_ways - 1) as u64)) as u8
}

// =============================================================================
// Dataset and Cache Specifications
// =============================================================================

/// Dataset node size range in bytes.
///
/// A dataset consists of NUM_NODES fixed-size nodes.
/// Node size affects memory bandwidth utilization and cache efficiency.
pub struct DatasetSpec;

impl DatasetSpec {
    /// Minimum node size in bytes (64 KiB).
    pub const NODE_SIZE_MIN: usize = 64 * 1024;

    /// Maximum node size in bytes (16 MiB).
    pub const NODE_SIZE_MAX: usize = 16 * 1024 * 1024;

    /// Minimum number of nodes in dataset.
    pub const NUM_NODES_MIN: usize = 16;

    /// Maximum number of nodes in dataset.
    pub const NUM_NODES_MAX: usize = 1024;

    /// Minimum total dataset memory (1 MiB).
    pub const MEMORY_MIN: usize = 1024 * 1024;

    /// Maximum total dataset memory (1 GiB).
    pub const MEMORY_MAX: usize = 1024 * 1024 * 1024;
}

/// Cache specification for light verification mode.
///
/// The cache enables verification without storing the full dataset.
/// Cache size is proportional to dataset size for efficiency.
pub struct CacheSpec;

impl CacheSpec {
    /// Minimum cache-to-dataset memory ratio (cache is at least 1/8 of dataset).
    pub const CACHE_MEMORY_RATIO_MIN: usize = 8;

    /// Maximum cache-to-dataset memory ratio (cache is at most 1/2 of dataset).
    pub const CACHE_MEMORY_RATIO_MAX: usize = 2;
}

// =============================================================================
// Domain Separators
// =============================================================================

/// Domain separator for epoch seed computation.
///
/// Format: "evo_omap_epoch" || epoch_number
pub const DOMAIN_EPOCH: &[u8] = b"evo_omap_epoch";

/// Domain separator for dataset node generation.
///
/// Format: "evo_omap_node" || epoch_seed || previous_node || index
pub const DOMAIN_NODE: &[u8] = b"evo_omap_node";

/// Domain separator for mining seed computation.
///
/// Format: "evo_omap_seed" || header || height || nonce
pub const DOMAIN_SEED: &[u8] = b"evo_omap_seed";

/// Domain separator for cache generation.
///
/// Format: "evo_omap_cache" || epoch_seed || block_index
pub const DOMAIN_CACHE: &[u8] = b"evo_omap_cache";

/// Domain separator for branch state derivation.
///
/// Format: "evo_omap_branch" || step || variant || state_bytes
pub const DOMAIN_BRANCH: &[u8] = b"evo_omap_branch";

/// Domain separator for rolling commitment.
///
/// Format: "evo_omap_commitment" || height || state_prefix_0 || ...
pub const DOMAIN_COMMITMENT: &[u8] = b"evo_omap_commitment";

/// Domain separator for memory Merkle commitment.
///
/// Leaf format:
/// prefixed("evo_omap_memory") || "leaf" || index_u64_le || node_len_u64_le || node
///
/// Parent format:
/// prefixed("evo_omap_memory") || "parent" || left_hash || right_hash
pub const DOMAIN_MEMORY: &[u8] = b"evo_omap_memory";

// =============================================================================
// Security Properties
// =============================================================================

/// # Security Properties
///
/// EVO-OMAP is designed with the following security properties:
///
/// 1. **Memory Hardness**: The algorithm requires a large, mutable dataset
///    that must be stored in RAM. This makes ASIC implementations
///    expensive due to SRAM requirements.
///
/// 2. **ASIC Resistance**: Data-dependent branching, rotations, and multiplies
///    prevent efficient fixed pipelines. GPUs suffer from warp divergence.
///    ASICs need complex branch units and barrel shifters.
///
/// 3. **CPU Fairness**: Modern CPUs with branch prediction handle the
///    4-way branching efficiently. The working set fits in L3 cache
///    for the arithmetic operations.
///
/// 4. **Dataset Chaining**: Sequential node generation prevents parallel
///    precomputation. Each node depends on the previous one.
///
/// 5. **State Entanglement**: Each step's branch result XORs into the
///    state, ensuring no step can be skipped or parallelized.
///
/// 6. **Cryptographic Diversity**: Using Blake3 for inner operations and
///    SHA3-256 for finalization prevents cross-domain attacks.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_execution() {
        let mut state = [1u64, 2, 3, 4, 5, 6, 7, 8];
        let node1_words = vec![0xFFu64; 128];
        let node2_words = vec![0xAAu64; 128];

        let add = Instruction::Add { dst: 0, src: 0 };
        add.execute(&mut state, &node1_words, &node2_words);

        assert_ne!(state[0], 1);
    }

    #[test]
    fn test_branch_variant_selection() {
        let state = [0xFFFFFFFFFFFFFFFFu64, 0, 0, 0, 0, 0, 0, 0];
        let variant = select_branch_variant(&state, 4);
        assert_eq!(variant, 3);

        let state = [0x0000000000000000u64, 0, 0, 0, 0, 0, 0, 0];
        let variant = select_branch_variant(&state, 4);
        assert_eq!(variant, 0);
    }

    #[test]
    fn test_instruction_swap() {
        let mut state = [1u64, 2, 3, 4, 5, 6, 7, 8];
        let node1_words = vec![0u64; 128];
        let node2_words = vec![0u64; 128];

        let swap = Instruction::Swap { a: 1, b: 2 };
        swap.execute(&mut state, &node1_words, &node2_words);

        assert_eq!(state[0], 1);
        assert_eq!(state[1], 3);
        assert_eq!(state[2], 2);
    }
}
