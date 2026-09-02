use crate::error::Result;
use crate::engine::EngineOutput;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

use super::types::*;

#[derive(Debug, Clone)]
pub struct PassengerStore {
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HeadInfo {
    pub passenger_version: String,
    pub version_dir: PathBuf,
    pub head_kind: HeadKind,
    pub branch: String,
    pub head_commit: Option<String>,
}

#[derive(Debug, Clone)]
pub enum HeadKind {
    Ref,
    Detached,
}

#[derive(Debug, Clone)]
pub struct CheckpointOptions {
    pub note: Option<String>,
    pub branch: Option<String>,        // override target branch (otherwise HEAD)
    pub include_artifacts: bool,       // write scan+analysis json
    pub track_roots: Option<Vec<String>>, // override config track_roots
}

impl Default for CheckpointOptions {
    fn default() -> Self {
        Self {
            note: None,
            branch: None,
            include_artifacts: true,
            track_roots: None,
        }
    }
}

impl PassengerStore {
    pub fn passenger_dir(root: &Path) -> PathBuf {
        root.join(".passenger")
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let p = Self::passenger_dir(&root);
        if !p.exists() {
            // keep it blunt: if not initialized, user should run init
            // (or you can auto-init, but explicit is safer)
            return Err(crate::error::PassengerError::Path(
                "missing .passenger (run: code-passenger passenger init)".into(),
            ));
        }
        Ok(Self { root })
    }

    pub fn init(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let p = Self::passenger_dir(&root);

        fs::create_dir_all(p.join("objects/sha256"))?;
        fs::create_dir_all(p.join("snapshots"))?;

        // config.toml
        let cfg_path = p.join("config.toml");
        if !cfg_path.exists() {
            let cfg = PassengerConfig::default();
            write_toml_atomic(&cfg_path, &cfg)?;
        }

        // state.json
        let st_path = p.join("state.json");
        if !st_path.exists() {
            let mut st = PassengerState::default();
            let cfg = read_toml::<PassengerConfig>(&cfg_path)?;
            st.next_seq.insert(cfg.passenger_version.clone(), 1);
            write_json_atomic(&st_path, &st)?;
        }

        // Ensure version dirs exist + refs
        let store = Self { root };
        store.ensure_version_layout()?;
        Ok(store)
    }

    pub fn config_path(&self) -> PathBuf {
        Self::passenger_dir(&self.root).join("config.toml")
    }
    pub fn state_path(&self) -> PathBuf {
        Self::passenger_dir(&self.root).join("state.json")
    }
    pub fn objects_dir(&self) -> PathBuf {
        Self::passenger_dir(&self.root).join("objects/sha256")
    }

    pub fn read_config(&self) -> Result<PassengerConfig> {
        read_toml(&self.config_path())
    }
    pub fn read_state(&self) -> Result<PassengerState> {
        read_json(&self.state_path())
    }
    pub fn write_state(&self, st: &PassengerState) -> Result<()> {
        write_json_atomic(&self.state_path(), st)
    }

    pub fn version_dir(&self, passenger_version: &str) -> PathBuf {
        Self::passenger_dir(&self.root)
            .join("snapshots")
            .join(format!("V{passenger_version}"))
    }

    pub fn ensure_version_layout(&self) -> Result<()> {
        let cfg = self.read_config()?;
        let vd = self.version_dir(&cfg.passenger_version);

        fs::create_dir_all(vd.join("refs"))?;
        fs::create_dir_all(vd.join("commits"))?;
        fs::create_dir_all(vd.join("artifacts"))?;
        fs::create_dir_all(vd.join("index"))?;

        // default branch ref
        let ref_path = vd.join("refs").join(&cfg.default_branch);
        if !ref_path.exists() {
            write_text_atomic(&ref_path, "")?; // empty means no commits yet
        }

        // HEAD
        let head_path = vd.join("HEAD");
        if !head_path.exists() {
            write_text_atomic(&head_path, format!("ref: refs/{}", cfg.default_branch))?;
        }

        Ok(())
    }

    pub fn resolve_head(&self) -> Result<HeadInfo> {
        let cfg = self.read_config()?;
        let vd = self.version_dir(&cfg.passenger_version);
        let head_path = vd.join("HEAD");
        let head_txt = fs::read_to_string(&head_path)?.trim().to_string();

        if let Some(rest) = head_txt.strip_prefix("ref: ") {
            let rest = rest.trim();
            let branch = rest.strip_prefix("refs/").unwrap_or(rest).to_string();
            let ref_path = vd.join("refs").join(&branch);
            let commit = fs::read_to_string(&ref_path).ok().map(|s| s.trim().to_string());
            let head_commit = commit.filter(|s| !s.is_empty());

            Ok(HeadInfo {
                passenger_version: cfg.passenger_version,
                version_dir: vd,
                head_kind: HeadKind::Ref,
                branch,
                head_commit,
            })
        } else {
            let detached = head_txt.trim().to_string();
            Ok(HeadInfo {
                passenger_version: cfg.passenger_version,
                version_dir: vd,
                head_kind: HeadKind::Detached,
                branch: "(detached)".to_string(),
                head_commit: if detached.is_empty() { None } else { Some(detached) },
            })
        }
    }

    pub fn create_branch(&self, name: &str, from: Option<&str>) -> Result<()> {
        let head = self.resolve_head()?;
        let refs_dir = head.version_dir.join("refs");
        fs::create_dir_all(&refs_dir)?;

        let new_ref = refs_dir.join(name);
        if new_ref.exists() {
            return Err(crate::error::PassengerError::Path(format!("branch already exists: {name}")));
        }

        let base = match from {
            Some(id) => id.to_string(),
            None => head.head_commit.clone().unwrap_or_default(),
        };

        write_text_atomic(&new_ref, base)?;
        Ok(())
    }

    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        let head = self.resolve_head()?;
        let ref_path = head.version_dir.join("refs").join(name);
        if !ref_path.exists() {
            return Err(crate::error::PassengerError::Path(format!("no such branch: {name}")));
        }
        let head_path = head.version_dir.join("HEAD");
        write_text_atomic(&head_path, format!("ref: refs/{name}"))?;
        Ok(())
    }

    pub fn detach_head(&self, snapshot_id: &str) -> Result<()> {
        let head = self.resolve_head()?;
        let head_path = head.version_dir.join("HEAD");
        write_text_atomic(&head_path, snapshot_id.to_string())?;
        Ok(())
    }

    pub fn read_commit(&self, passenger_version: &str, id: &str) -> Result<PassengerCommit> {
        let p = self.version_dir(passenger_version).join("commits").join(format!("{id}.json"));
        read_json(&p)
    }

    pub fn checkpoint(
        &self,
        raw: Option<&EngineOutput>,
        analysis_json: Option<&serde_json::Value>,
        opts: CheckpointOptions,
    ) -> Result<PassengerCommit> {
        self.ensure_version_layout()?;

        let cfg = self.read_config()?;
        let mut st = self.read_state()?;
        let head = self.resolve_head()?;

        let target_branch = opts.branch.clone().unwrap_or_else(|| {
            if matches!(head.head_kind, HeadKind::Ref) {
                head.branch.clone()
            } else {
                cfg.default_branch.clone()
            }
        });

        // ensure branch exists
        let ref_path = head.version_dir.join("refs").join(&target_branch);
        if !ref_path.exists() {
            // auto-create branch from current head commit
            self.create_branch(&target_branch, head.head_commit.as_deref())?;
        }

        // parent commit (from branch ref)
        let parent_id = fs::read_to_string(&ref_path).ok().map(|s| s.trim().to_string());
        let parent_id = parent_id.filter(|s| !s.is_empty());

        // allocate next snapshot id
        let seq = st.next_seq.entry(cfg.passenger_version.clone()).or_insert(1);
        let id = format!("S{:06}", *seq);
        *seq += 1;
        self.write_state(&st)?;

        // build full manifest over tracked roots
        let track_roots = opts.track_roots.clone().unwrap_or_else(|| cfg.track_roots.clone());
        let manifest = self.build_full_manifest(&track_roots)?;

        // compute stats vs parent (changed_files + line diffs)
        let stats = self.compute_stats_delta(&cfg, parent_id.as_deref(), &manifest)?;

        // build commit (hash filled after serialization)
        let mut commit = PassengerCommit {
            schema: 1,
            id: id.clone(),
            ts_ms: chrono::Utc::now().timestamp_millis(),
            passenger_version: cfg.passenger_version.clone(),
            branch: target_branch.clone(),
            parents: parent_id.clone().into_iter().collect(),
            manifest,
            stats,
            note: opts.note.clone(),
            prev_hash: None,
            hash: String::new(),
        };

        // chain hash: prev_hash = parent.hash, commit.hash = sha256(json_without_hash + prev_hash)
        if let Some(pid) = &parent_id {
            let pc = self.read_commit(&cfg.passenger_version, pid)?;
            commit.prev_hash = Some(pc.hash);
        }

        let commit_hash = compute_commit_hash(&commit)?;
        commit.hash = commit_hash;

        // write commit
        let commit_path = head
            .version_dir
            .join("commits")
            .join(format!("{id}.json"));
        write_json_atomic(&commit_path, &commit)?;

        // update ref -> new commit
        write_text_atomic(&ref_path, id.clone())?;

        // artifacts (optional): save scan + analysis
        if opts.include_artifacts {
            let art_dir = head.version_dir.join("artifacts").join(&id);
            fs::create_dir_all(&art_dir)?;
            if let Some(raw) = raw {
                write_json_atomic(&art_dir.join("scan.json"), raw)?;
            }
            if let Some(aj) = analysis_json {
                write_json_atomic(&art_dir.join("analysis.json"), aj)?;
            }
        }

        Ok(commit)
    }

    fn build_full_manifest(&self, track_roots: &[String]) -> Result<Manifest> {
        let mut files: BTreeMap<String, FileEntry> = BTreeMap::new();

        for root in track_roots {
            let root_path = self.root.join(root);
            if !root_path.exists() {
                continue;
            }

            if root_path.is_file() {
                let rel = root.to_string();
                let entry = self.ingest_file(&root_path)?;
                files.insert(rel, entry);
                continue;
            }

            // directory walk
            for ent in WalkDir::new(&root_path).into_iter().filter_map(|e| e.ok()) {
                if !ent.file_type().is_file() {
                    continue;
                }

                let p = ent.path();

                // ignore .passenger and target everywhere
                if p.components().any(|c| c.as_os_str() == ".passenger") { continue; }
                if p.components().any(|c| c.as_os_str() == "target") { continue; }

                // compute rel path from repo root
                let rel = p.strip_prefix(&self.root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .to_string();

                let entry = self.ingest_file(p)?;
                files.insert(rel, entry);
            }
        }

        Ok(Manifest {
            kind: ManifestKind::Full,
            base: None,
            files,
            deleted: vec![],
        })
    }

    fn ingest_file(&self, path: &Path) -> Result<FileEntry> {
        let bytes = fs::read(path)?;
        let hash = sha256_hex(&bytes);

        // store blob if missing
        self.write_object_if_missing(&hash, &bytes)?;

        let lines = count_lines_bytes(&bytes);
        Ok(FileEntry {
            hash,
            bytes: bytes.len() as u64,
            lines: lines as u64,
        })
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        let a = &hash[0..2];
        let b = &hash[2..4];
        self.objects_dir().join(a).join(b).join(format!("{hash}.zst"))
    }

    fn write_object_if_missing(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        let p = self.object_path(hash);
        if p.exists() {
            return Ok(());
        }
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }

        // always store as zstd (simple) — config.compress could toggle raw later
        let compressed = zstd::encode_all(bytes, 3)?;
        write_bytes_atomic(&p, &compressed)?;
        Ok(())
    }

    fn read_object(&self, hash: &str) -> Result<Vec<u8>> {
        let p = self.object_path(hash);
        let compressed = fs::read(&p)?;
        let decoded = zstd::decode_all(&compressed[..])?;
        Ok(decoded)
    }

    fn compute_stats_delta(
        &self,
        cfg: &PassengerConfig,
        parent_id: Option<&str>,
        new_manifest: &Manifest,
    ) -> Result<CommitStats> {
        let mut st = CommitStats::default();

        let Some(pid) = parent_id else {
            st.changed_files = new_manifest.files.len();
            return Ok(st);
        };

        let parent = self.read_commit(&cfg.passenger_version, pid)?;
        let old = &parent.manifest.files;
        let new = &new_manifest.files;

        // changed files
        let mut changed_paths: Vec<String> = vec![];
        for (p, e) in new {
            match old.get(p) {
                Some(oe) if oe.hash == e.hash => {}
                _ => changed_paths.push(p.clone()),
            }
        }
        for p in old.keys() {
            if !new.contains_key(p) {
                changed_paths.push(p.clone());
            }
        }
        changed_paths.sort();
        changed_paths.dedup();
        st.changed_files = changed_paths.len();

        // line diffs for changed files (best-effort)
        for p in changed_paths {
            let old_hash = old.get(&p).map(|x| x.hash.clone());
            let new_hash = new.get(&p).map(|x| x.hash.clone());

            match (old_hash, new_hash) {
                (Some(oh), Some(nh)) => {
                    let old_txt = String::from_utf8_lossy(&self.read_object(&oh)?).to_string();
                    let new_txt = String::from_utf8_lossy(&self.read_object(&nh)?).to_string();
                    let (a, r) = diff_line_counts(&old_txt, &new_txt);
                    st.added_lines += a;
                    st.removed_lines += r;
                }
                (None, Some(nh)) => {
                    let new_txt = String::from_utf8_lossy(&self.read_object(&nh)?).to_string();
                    st.added_lines += new_txt.lines().count();
                }
                (Some(oh), None) => {
                    let old_txt = String::from_utf8_lossy(&self.read_object(&oh)?).to_string();
                    st.removed_lines += old_txt.lines().count();
                }
                (None, None) => {}
            }
        }

        // placeholder: later you can compute from editor event logs / heuristics
        st.paste_score = 0.0;

        Ok(st)
    }
}

/* ----------------------- helpers ----------------------- */

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn count_lines_bytes(bytes: &[u8]) -> usize {
    // cheap + stable (counts '\n', plus 1 if non-empty & no trailing newline)
    if bytes.is_empty() { return 0; }
    let mut n = bytes.iter().filter(|b| **b == b'\n').count();
    if *bytes.last().unwrap() != b'\n' {
        n += 1;
    }
    n
}

fn diff_line_counts(old_txt: &str, new_txt: &str) -> (usize, usize) {
    use similar::TextDiff;
    let diff = TextDiff::from_lines(old_txt, new_txt);

    let mut add = 0usize;
    let mut rem = 0usize;

    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            match change.tag() {
                similar::ChangeTag::Insert => add += 1,
                similar::ChangeTag::Delete => rem += 1,
                similar::ChangeTag::Equal => {}
            }
        }
    }
    (add, rem)
}

fn compute_commit_hash(c: &PassengerCommit) -> Result<String> {
    // hash commit json *without* the `hash` field content
    // simplest: clone, blank hash, serialize
    let mut tmp = c.clone();
    tmp.hash.clear();

    let json = serde_json::to_vec(&tmp)?;
    let mut h = Sha256::new();
    h.update(&json);

    if let Some(prev) = &c.prev_hash {
        h.update(prev.as_bytes());
    }

    Ok(hex::encode(h.finalize()))
}

fn read_json<T: for<'de> serde::Deserialize<'de>>(p: &Path) -> Result<T> {
    let bytes = fs::read(p)?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn write_json_atomic<T: serde::Serialize>(p: &Path, v: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(v)?;
    write_bytes_atomic(p, &bytes)
}

fn read_toml<T: for<'de> serde::Deserialize<'de>>(p: &Path) -> Result<T> {
    let s = fs::read_to_string(p)?;
    Ok(toml::from_str(&s)?)
}
fn write_toml_atomic<T: serde::Serialize>(p: &Path, v: &T) -> Result<()> {
    let s = toml::to_string_pretty(v).unwrap();
    write_text_atomic(p, s)
}

fn write_text_atomic(p: &Path, s: impl Into<String>) -> Result<()> {
    write_bytes_atomic(p, s.into().as_bytes())
}

fn write_bytes_atomic(p: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = p.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, p)?;
    Ok(())
}
