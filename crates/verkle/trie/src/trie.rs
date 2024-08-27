use std::sync::Mutex;
use once_cell::sync::Lazy;
use verkle_trie::{database::memory_db::MemoryDb, Trie, TrieTrait, VerkleConfig};