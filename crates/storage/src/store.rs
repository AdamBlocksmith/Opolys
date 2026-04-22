use opolys_core::{Block, ObjectId, Hash, OpolysError};
use opolys_consensus::account::AccountStore;
use opolys_consensus::pos::ValidatorSet;
use borsh::{BorshSerialize, BorshDeserialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreCF {
    Blocks,
    Accounts,
    Validators,
    ChainState,
    Transactions,
}

impl StoreCF {
    pub fn name(&self) -> &'static str {
        match self {
            StoreCF::Blocks => "blocks",
            StoreCF::Accounts => "accounts",
            StoreCF::Validators => "validators",
            StoreCF::ChainState => "chain_state",
            StoreCF::Transactions => "transactions",
        }
    }

    pub fn all() -> &'static [StoreCF] {
        &[
            StoreCF::Blocks,
            StoreCF::Accounts,
            StoreCF::Validators,
            StoreCF::ChainState,
            StoreCF::Transactions,
        ]
    }
}

pub struct OpolysStore {
    db: rocksdb::DB,
}

impl OpolysStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);

        for cf in StoreCF::all() {
            opts.create_missing_column_families(true);
        }

        let cf_names: Vec<&str> = StoreCF::all().iter().map(|cf| cf.name()).collect();
        let db = rocksdb::DB::open_cf(&opts, path, cf_names)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        Ok(OpolysStore { db })
    }

    pub fn put(&self, cf: StoreCF, key: &[u8], value: &[u8]) -> Result<(), String> {
        let handle = self.db.cf_handle(cf.name())
            .ok_or_else(|| format!("Column family not found: {}", cf.name()))?;
        self.db.put_cf(handle, key, value)
            .map_err(|e| format!("Put failed: {}", e))
    }

    pub fn get(&self, cf: StoreCF, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let handle = self.db.cf_handle(cf.name())
            .ok_or_else(|| format!("Column family not found: {}", cf.name()))?;
        self.db.get_cf(handle, key)
            .map_err(|e| format!("Get failed: {}", e))
    }

    pub fn delete(&self, cf: StoreCF, key: &[u8]) -> Result<(), String> {
        let handle = self.db.cf_handle(cf.name())
            .ok_or_else(|| format!("Column family not found: {}", cf.name()))?;
        self.db.delete_cf(handle, key)
            .map_err(|e| format!("Delete failed: {}", e))
    }

    pub fn put_borsh<T: BorshSerialize>(&self, cf: StoreCF, key: &[u8], value: &T) -> Result<(), String> {
        let data = borsh::to_vec(value).map_err(|e| format!("Serialization failed: {}", e))?;
        self.put(cf, key, &data)
    }

    pub fn get_borsh<T: BorshDeserialize>(&self, cf: StoreCF, key: &[u8]) -> Result<Option<T>, String> {
        if let Some(data) = self.get(cf, key)? {
            let value = T::try_from_slice(&data).map_err(|e| format!("Deserialization failed: {}", e))?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub fn exists(&self, cf: StoreCF, key: &[u8]) -> Result<bool, String> {
        self.get(cf, key).map(|v| v.is_some())
    }

    pub fn write_batch(&self, batch: Vec<(StoreCF, Vec<u8>, Vec<u8>)>) -> Result<(), String> {
        let mut wb = rocksdb::WriteBatch::default();
        for (cf, key, value) in batch {
            let handle = self.db.cf_handle(cf.name())
                .ok_or_else(|| format!("Column family not found: {}", cf.name()))?;
            wb.put_cf(handle, key, value);
        }
        self.db.write(wb).map_err(|e| format!("Write batch failed: {}", e))
    }
}