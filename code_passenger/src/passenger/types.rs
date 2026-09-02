use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassengerConfig {
    pub schema: u32,
    pub passenger_version: String,  // e.g. "0.3.1"
    pub default_branch: String,     // e.g. "main"
    pub hash_algo: String,          // "sha256"
    pub compress: bool,             // store blobs compressed
    pub track_roots: Vec<String>,   // e.g. ["src", "Cargo.toml"]
}

impl Default for PassengerConfig {
    fn default() -> Self {
        Self {
            schema: 1,
            passenger_version: "0.1.0".to_string(),
            default_branch: "main".to_string(),
            hash_algo: "sha256".to_string(),
            compress: true,
            track_roots: vec!["src".to_string(), "Cargo.toml".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassengerState {
    pub schema: u32,
    pub created_ms: i64,
    /// passenger_version -> next sequence integer
    pub next_seq: BTreeMap<String, u64>,
}

impl Default for PassengerState {
    fn default() -> Self {
        Self {
            schema: 1,
            created_ms: chrono::Utc::now().timestamp_millis(),
            next_seq: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassengerCommit {
    pub schema: u32,
    pub id: String,                 // "S000042"
    pub ts_ms: i64,
    pub passenger_version: String,
    pub branch: String,
    pub parents: Vec<String>,
    pub manifest: Manifest,
    pub stats: CommitStats,
    pub note: Option<String>,
    pub prev_hash: Option<String>,
    pub hash: String,               // hash of this commit content (chainable)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub kind: ManifestKind,
    pub base: Option<String>, // if delta
    pub files: BTreeMap<String, FileEntry>, // rel_path -> entry
    pub deleted: Vec<String>, // if delta
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ManifestKind {
    Full,
    Delta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub hash: String,   // sha256 hex
    pub bytes: u64,
    pub lines: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommitStats {
    pub changed_files: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub paste_score: f32,   // placeholder for later
}
