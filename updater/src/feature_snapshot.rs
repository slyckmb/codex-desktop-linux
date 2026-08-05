//! Durable Linux-feature selections bound to an update candidate.
//!
//! A user may change the feature picker between detection and installation.
//! Candidate rebuilds must instead use the exact selection reviewed when that
//! candidate was recorded, so snapshots are immutable and keyed by the DMG
//! hash or wrapper commit.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{config::RuntimePaths, state};

const SNAPSHOT_DIR: &str = "feature-snapshots";
const EMPTY_FEATURES: &[u8] = br#"{"enabled":[]}"#;

pub fn capture(
    paths: &RuntimePaths,
    candidate_key: &str,
    source: Option<&Path>,
) -> Result<PathBuf> {
    let path = path_for(paths, candidate_key);
    if path.exists() {
        return Ok(path);
    }
    let bytes = match source {
        Some(source) => fs::read(source)
            .with_context(|| format!("Failed to read feature config {}", source.display()))?,
        None => EMPTY_FEATURES.to_vec(),
    };
    let dir = path.parent().expect("snapshot path has parent");
    fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    state::atomic_write(&path, &bytes)?;
    Ok(path)
}

pub fn path_for(paths: &RuntimePaths, candidate_key: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(candidate_key.as_bytes());
    let digest = hasher.finalize();
    let name = format!(
        "{}.json",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    paths.state_dir.join(SNAPSHOT_DIR).join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn paths(root: &Path) -> RuntimePaths {
        RuntimePaths {
            config_file: root.join("config/config.toml"),
            state_file: root.join("state/state.json"),
            log_file: root.join("state/service.log"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            config_dir: root.join("config"),
        }
    }

    #[test]
    fn capture_is_immutable_for_a_candidate() -> Result<()> {
        let temp = tempdir()?;
        let source = temp.path().join("features.json");
        fs::write(&source, br#"{"enabled":["first"]}"#)?;
        let paths = paths(temp.path());
        let snapshot = capture(&paths, "wrapper:abc", Some(&source))?;
        fs::write(&source, br#"{"enabled":["second"]}"#)?;
        assert_eq!(snapshot, capture(&paths, "wrapper:abc", Some(&source))?);
        assert_eq!(fs::read(&snapshot)?, br#"{"enabled":["first"]}"#);
        Ok(())
    }
}
