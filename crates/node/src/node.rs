use opolys_core::*;
use opolys_consensus::{
    account::AccountStore,
    difficulty::DifficultyTarget,
    emission,
    mempool::Mempool,
    pos::ValidatorSet,
    pow,
    genesis::GenesisConfig,
};
use opolys_consensus::difficulty::{compute_next_difficulty, compute_consensus_floor, compute_discovery_bonus};
use opolys_consensus::block::{compute_transaction_root, BlockInfo};
use opolys_execution::TransactionDispatcher;
use std::sync::Arc;
use tokio::sync::RwLock;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "opolys", about = "Opolys blockchain node")]
pub struct Args {
    #[arg(long, default_value = "4170")]
    pub port: u16,

    #[arg(long)]
    pub rpc_port: Option<u16>,

    #[arg(long)]
    pub data_dir: Option<String>,

    #[arg(long)]
    pub bootstrap: Option<String>,

    #[arg(long, default_value = "info")]
    pub log_level: String,
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub listen_port: u16,
    pub rpc_port: u16,
    pub data_dir: String,
    pub bootstrap_peers: Vec<String>,
    pub log_level: String,
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            listen_port: DEFAULT_LISTEN_PORT,
            rpc_port: DEFAULT_LISTEN_PORT + 1,
            data_dir: "./data".to_string(),
            bootstrap_peers: vec![],
            log_level: "info".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChainState {
    pub current_height: u64,
    pub current_difficulty: u64,
    pub total_issued: FleckAmount,
    pub total_burned: FleckAmount,
    pub block_timestamps: Vec<u64>,
    pub latest_block_hash: Hash,
    pub state_root: Hash,
    pub phase: ConsensusPhase,
}

impl ChainState {
    pub fn new(genesis_config: &GenesisConfig) -> Self {
        let genesis = opolys_consensus::build_genesis_block(genesis_config);
        ChainState {
            current_height: 0,
            current_difficulty: genesis_config.initial_difficulty,
            total_issued: 0,
            total_burned: 0,
            block_timestamps: vec![0],
            latest_block_hash: Hash::zero(),
            state_root: genesis.header.state_root.clone(),
            phase: ConsensusPhase::ProofOfWork,
        }
    }

    pub fn circulating_supply(&self) -> FleckAmount {
        self.total_issued.saturating_sub(self.total_burned)
    }

    pub fn stake_coverage(&self) -> f64 {
        emission::compute_stake_coverage(
            self.total_issued,
            self.total_issued,
        )
    }
}

pub struct OpolysNode {
    pub chain: Arc<RwLock<ChainState>>,
    pub accounts: Arc<RwLock<AccountStore>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub validators: Arc<RwLock<ValidatorSet>>,
    pub config: NodeConfig,
}

impl OpolysNode {
    pub fn new(config: NodeConfig) -> Self {
        let genesis_config = GenesisConfig::default();
        let chain_state = ChainState::new(&genesis_config);

        OpolysNode {
            chain: Arc::new(RwLock::new(chain_state)),
            accounts: Arc::new(RwLock::new(AccountStore::new())),
            mempool: Arc::new(RwLock::new(Mempool::new())),
            validators: Arc::new(RwLock::new(ValidatorSet::new())),
            config,
        }
    }

    pub async fn mine_block(&self, max_attempts: u64) -> Option<Block> {
        let chain = self.chain.read().await;
        let accounts = self.accounts.read().await;
        let validators = self.validators.read().await;

        let mempool = self.mempool.read().await;
        let transactions: Vec<Transaction> = mempool.get_ordered_transactions()
            .into_iter()
            .take(100)
            .cloned()
            .collect();

        let transaction_root = compute_transaction_root(&transactions);
        let bonded_stake = validators.total_bonded_stake();
        let total_issued = chain.total_issued;

        let diff_target = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            total_issued,
            bonded_stake,
        );

        let difficulty = diff_target.effective_difficulty();

        let mut header = BlockHeader {
            height: chain.current_height + 1,
            previous_hash: chain.latest_block_hash.clone(),
            state_root: chain.state_root.clone(),
            transaction_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            difficulty,
            pow_proof: None,
            validator_signature: None,
        };

        drop(chain);
        drop(accounts);
        drop(validators);
        drop(mempool);

        let block = pow::mine_block(header, difficulty, max_attempts)?;

        Some(block)
    }

    pub async fn apply_block(&self, block: &Block) -> Result<(), String> {
        let mut chain = self.chain.write().await;
        let mut accounts = self.accounts.write().await;
        let mut validators = self.validators.write().await;
        let mut mempool = self.mempool.write().await;

        let bonded_stake = validators.total_bonded_stake();
        let stake_coverage = emission::compute_stake_coverage(bonded_stake, chain.total_issued);

        let pow_hash = if let Some(ref proof) = block.header.pow_proof {
            let nonce = u64::from_be_bytes(proof[..8].try_into().unwrap_or([0u8; 8]));
            let dataset = pow::AutolykosDataset::generate(&[], block.header.height);
            let hash = pow::autolykos_hash(&dataset, &block.header, nonce);
            u64::from_be_bytes(hash.0[..8].try_into().unwrap_or([0u8; 8]))
        } else {
            0u64
        };

        let discovery_bonus = compute_discovery_bonus(block.header.difficulty, pow_hash);
        let block_reward = emission::compute_block_reward(block.header.difficulty, discovery_bonus);

        chain.total_issued = chain.total_issued.saturating_add(block_reward);
        chain.current_height = block.header.height;
        chain.current_difficulty = block.header.difficulty;
        chain.latest_block_hash = Hash::from_bytes([0u8; 64]);
        chain.block_timestamps.push(block.header.timestamp);

        let mut total_fees_burned: FleckAmount = 0;
        for tx in &block.transactions {
            let result = TransactionDispatcher::apply_transaction(
                tx,
                &mut accounts,
                &mut validators,
                block.header.height,
                block.header.timestamp,
            );
            if result.success {
                total_fees_burned = total_fees_burned.saturating_add(result.fee_burned);
            }
            mempool.remove_transaction(&tx.tx_id);
        }

        chain.total_burned = chain.total_burned.saturating_add(total_fees_burned);

        

        Ok(())
    }

    pub fn get_block(&self, height: u64) -> Option<Block> {
        None
    }
}

pub struct RpcContextImpl {
    pub chain: Arc<RwLock<ChainState>>,
    pub accounts: Arc<RwLock<AccountStore>>,
    pub validators: Arc<RwLock<ValidatorSet>>,
}

impl RpcContextImpl {
    pub fn new(node: &OpolysNode) -> Self {
        RpcContextImpl {
            chain: node.chain.clone(),
            accounts: node.accounts.clone(),
            validators: node.validators.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_initialization() {
        let config = NodeConfig::default();
        let node = OpolysNode::new(config);
        assert_eq!(node.chain.blocking_read().current_height, 0);
    }

    #[tokio::test]
    async fn chain_state_circulating_supply() {
        let config = NodeConfig::default();
        let genesis_config = GenesisConfig::default();
        let chain = ChainState::new(&genesis_config);
        assert_eq!(chain.circulating_supply(), 0);
    }
}