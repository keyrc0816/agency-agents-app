//! Validated display-only agent localization metadata.
//!
//! Canonical English metadata and persona bodies remain authoritative. Invalid,
//! missing, or stale rows are omitted so every consumer naturally falls back to
//! English without affecting identity, paths, or catalog synchronization.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::types::{Agent, AgentLocalization};

const SCHEMA_VERSION: u32 = 1;
const LOCALE: &str = "zh-TW";
const RESOURCE: &str = "scripts/i18n/agent-metadata-zh-TW.json";
const MAX_RESOURCE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationFile {
    schema_version: u32,
    locale: String,
    agents: BTreeMap<String, LocalizationRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationRow {
    source_name: String,
    source_description_sha256: String,
    name: String,
    description: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn clean_display(value: &str) -> bool {
    !value.trim().is_empty()
        && value == value.trim()
        && !value.contains(['\r', '\n'])
        && !value.chars().any(|ch| ch.is_control())
}

fn row_hash(slug: &str, row: &LocalizationRow) -> String {
    let material = format!("{LOCALE}\0{slug}\0{}\0{}", row.name, row.description);
    sha256_hex(material.as_bytes())
}

fn select_resource(active_root: &Path, fallback_root: &Path) -> PathBuf {
    let active = active_root.join(RESOURCE);
    if active.is_file() {
        active
    } else {
        fallback_root.join(RESOURCE)
    }
}

/// Apply only valid and current localizations. A malformed resource is a
/// recoverable error: the caller logs it and serves canonical English.
pub fn apply(
    active_root: &Path,
    fallback_root: &Path,
    agents: &mut [Agent],
) -> Result<usize, String> {
    let path = select_resource(active_root, fallback_root);
    let meta = std::fs::metadata(&path)
        .map_err(|e| format!("read localization metadata {}: {e}", path.display()))?;
    if meta.len() > MAX_RESOURCE_BYTES {
        return Err(format!(
            "localization metadata exceeds {MAX_RESOURCE_BYTES} bytes"
        ));
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("read localization metadata {}: {e}", path.display()))?;
    let file: LocalizationFile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("parse localization metadata {}: {e}", path.display()))?;
    if file.schema_version != SCHEMA_VERSION || file.locale != LOCALE {
        return Err(format!(
            "unsupported localization schema/locale: {}/{}",
            file.schema_version, file.locale
        ));
    }

    let mut applied = 0;
    for agent in agents {
        let Some(row) = file.agents.get(&agent.slug) else {
            continue;
        };
        let description_hash = sha256_hex(agent.description.as_bytes());
        if row.source_name != agent.name
            || row.source_description_sha256 != description_hash
            || !clean_display(&row.name)
            || !clean_display(&row.description)
        {
            continue;
        }
        agent.localizations.insert(
            LOCALE.to_string(),
            AgentLocalization {
                name: row.name.clone(),
                description: row.description.clone(),
                localization_hash: row_hash(&agent.slug, row),
            },
        );
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> Agent {
        Agent {
            slug: "ai-engineer".into(),
            name: "AI Engineer".into(),
            description: "Builds production ML systems.".into(),
            localizations: BTreeMap::new(),
            category: "engineering".into(),
            emoji: None,
            color: None,
            vibe: None,
            body: "CANONICAL BODY".into(),
        }
    }

    fn write_resource(root: &Path, source_hash: &str, description: &str) {
        let path = root.join(RESOURCE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = format!(
            r#"{{"schemaVersion":1,"locale":"zh-TW","agents":{{"ai-engineer":{{"sourceName":"AI Engineer","sourceDescriptionSha256":"{source_hash}","name":"AI 工程師","description":"{description}"}}}}}}"#
        );
        std::fs::write(path, json).unwrap();
    }

    #[test]
    fn valid_row_applies_without_touching_canonical_fields() {
        let root = tempfile::tempdir().unwrap();
        let mut agents = vec![agent()];
        let hash = sha256_hex(agents[0].description.as_bytes());
        write_resource(root.path(), &hash, "建置可上線的機器學習系統。");
        assert_eq!(apply(root.path(), root.path(), &mut agents).unwrap(), 1);
        assert_eq!(agents[0].name, "AI Engineer");
        assert_eq!(agents[0].body, "CANONICAL BODY");
        assert_eq!(agents[0].localizations[LOCALE].name, "AI 工程師");
    }

    #[test]
    fn stale_row_falls_back_to_english() {
        let root = tempfile::tempdir().unwrap();
        let mut agents = vec![agent()];
        write_resource(root.path(), &"0".repeat(64), "過期內容");
        assert_eq!(apply(root.path(), root.path(), &mut agents).unwrap(), 0);
        assert!(agents[0].localizations.is_empty());
    }

    #[test]
    fn malformed_resource_is_recoverable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(RESOURCE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"{not json").unwrap();
        let mut agents = vec![agent()];
        assert!(apply(root.path(), root.path(), &mut agents).is_err());
        assert!(agents[0].localizations.is_empty());
    }
}
