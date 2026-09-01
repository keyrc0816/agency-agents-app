//! Tolerant frontmatter parsing + canonical hashing for the corpus.
//!
//! Each agent lives in a single `.md` whose head is a YAML frontmatter
//! block fenced by `---` lines, followed by the markdown persona body:
//!
//! ```text
//! ---
//! name: Frontend Developer
//! description: "Builds delightful UIs."
//! color: blue
//! emoji: 🎨
//! vibe: Ships pixel-perfect interfaces.
//! ---
//! # Frontend Developer Agent
//! You are a …
//! ```
//!
//! Parsing mirrors the agency-agents `scripts/convert.sh` reference
//! (`get_field` / `get_body`): the frontmatter is everything between the
//! first two `---` fences; the body is everything after the second
//! fence. We parse the frontmatter with `serde_yaml` (tolerant of quoted
//! / multiline values) rather than the shell's line-grep so descriptions
//! that span lines or carry colons survive intact.
//!
//! Determinism (contracts.md §E): all three hashes are SHA-256 lowercase
//! hex of the UTF-8 bytes of a *canonical* slice of the source — no
//! timestamps, no re-serialization, no map reordering. We hash the raw
//! byte ranges of the original file so the same `.md` always yields the
//! same hashes regardless of platform or run.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::types::{Agent, CorpusEntry};

/// The subset of frontmatter keys we surface. Unknown keys are ignored
/// (tolerant parse). `name` is the only required field — a file without
/// it is not an agent (READMEs, workflow docs) and is skipped upstream.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    emoji: Option<String>,
    color: Option<String>,
    vibe: Option<String>,
}

const LEGACY_SCALAR_FIELDS: [&str; 5] = ["name", "description", "emoji", "color", "vibe"];

#[derive(Deserialize)]
struct LegacyScalar {
    value: Option<String>,
}

/// Recover the repository's legacy one-line scalar convention without making
/// the whole parser permissive. The converter treats everything after
/// `<field>: ` as human-readable text, including a later `: ` sequence. We
/// mirror that behavior only for the five surfaced metadata fields and only
/// after strict YAML rejected the complete frontmatter.
fn parse_legacy_frontmatter(frontmatter: &str) -> Result<Frontmatter, ()> {
    let mut fields: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut current: Option<&str> = None;

    for line in frontmatter.lines() {
        let is_continuation = line.starts_with(' ') || line.starts_with('\t');
        let is_blank_or_comment = line.trim().is_empty() || line.trim_start().starts_with('#');
        if is_continuation || is_blank_or_comment {
            if let Some(key) = current {
                fields.get_mut(key).ok_or(())?.push(line);
            } else if is_continuation {
                return Err(());
            }
            continue;
        }

        let (key, _) = line.split_once(": ").ok_or(())?;
        if !LEGACY_SCALAR_FIELDS.contains(&key) || fields.contains_key(key) {
            return Err(());
        }
        fields.insert(key, vec![line]);
        current = Some(key);
    }

    let mut parsed = Frontmatter::default();
    let mut recovered_legacy_scalar = false;
    for (key, lines) in fields {
        let (value, recovered) = parse_legacy_scalar(key, &lines)?;
        recovered_legacy_scalar |= recovered;
        match key {
            "name" => parsed.name = Some(value),
            "description" => parsed.description = Some(value),
            "emoji" => parsed.emoji = Some(value),
            "color" => parsed.color = Some(value),
            "vibe" => parsed.vibe = Some(value),
            _ => return Err(()),
        }
    }

    // A strict failure that did not require the approved colon compatibility
    // is unrelated malformed YAML and must retain the original error path.
    recovered_legacy_scalar.then_some(parsed).ok_or(())
}

fn parse_legacy_scalar(key: &str, lines: &[&str]) -> Result<(String, bool), ()> {
    let first = *lines.first().ok_or(())?;
    let prefix = format!("{key}:");
    let suffix = first.strip_prefix(&prefix).ok_or(())?;

    // Parse each otherwise-valid field with serde_yaml so quoted strings and
    // block/continuation semantics remain identical to the strict path.
    let mut single_field = format!("value:{suffix}");
    for line in &lines[1..] {
        single_field.push('\n');
        single_field.push_str(line);
    }
    if let Ok(value) = serde_yaml::from_str::<LegacyScalar>(&single_field) {
        return value.value.map(|value| (value, false)).ok_or(());
    }

    // The only non-YAML form accepted is the converter-compatible, one-line
    // plain scalar whose content itself contains `: `. Structured or quoted
    // malformed values, comments, controls, and continuations remain errors.
    if lines.len() != 1 {
        return Err(());
    }
    let raw = suffix.strip_prefix(' ').ok_or(())?.trim_end();
    if !is_safe_legacy_plain_scalar(raw) {
        return Err(());
    }
    Ok((raw.to_string(), true))
}

fn is_safe_legacy_plain_scalar(value: &str) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    value.contains(": ")
        && !first.is_whitespace()
        && !matches!(
            first,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
        && !value.contains(" #")
        && !value.chars().any(|c| c.is_control())
}

/// Result of splitting a raw `.md` into its three canonical regions.
/// Slices borrow from the source so hashing never copies.
struct Split<'a> {
    /// The frontmatter YAML, *between* the fences (no `---` lines).
    frontmatter: &'a str,
    /// The persona body, after the closing fence.
    body: &'a str,
}

/// Split a raw agent `.md` into frontmatter + body.
///
/// Returns `None` when there is no well-formed `---`-fenced frontmatter
/// block at the head of the file (the file is not an agent). We accept a
/// leading UTF-8 BOM and trailing whitespace on the fence lines so files
/// authored on different editors still parse.
fn split_frontmatter(source: &str) -> Option<Split<'_>> {
    // Tolerate a leading BOM.
    let s = source.strip_prefix('\u{feff}').unwrap_or(source);

    // The opening fence must be the very first line (after the optional
    // BOM). We match a line that is exactly `---` ignoring trailing
    // whitespace.
    let mut rest = s;
    let first_line_end = rest.find('\n')?;
    let first_line = rest[..first_line_end].trim_end();
    if first_line != "---" {
        return None;
    }
    rest = &rest[first_line_end + 1..];

    // Walk lines until the closing fence. We track byte offsets so the
    // frontmatter slice is exact.
    let fm_start_offset = rest.as_ptr() as usize - s.as_ptr() as usize;
    let mut search_from = 0usize;
    loop {
        let line_end = rest[search_from..]
            .find('\n')
            .map(|i| search_from + i)
            .unwrap_or(rest.len());
        let line = rest[search_from..line_end].trim_end();
        if line == "---" {
            let fm_end_offset = fm_start_offset + search_from;
            let frontmatter = &s[fm_start_offset..fm_end_offset];
            // Body starts after this closing-fence line's newline (if any).
            let body_offset = if line_end < rest.len() {
                fm_start_offset + line_end + 1
            } else {
                s.len()
            };
            let body = &s[body_offset..];
            return Some(Split { frontmatter, body });
        }
        if line_end >= rest.len() {
            // Hit EOF without a closing fence — malformed, not an agent.
            return None;
        }
        search_from = line_end + 1;
    }
}

/// SHA-256 lowercase hex of `bytes` (contracts.md §E rule 2).
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Parse one agent `.md`.
///
/// `slug` is the filename without `.md` (contracts.md §A); `category` is
/// the parent directory name. Returns `Ok(None)` for files that are not
/// agents (no frontmatter, or no `name`) so callers can skip them without
/// treating it as an error. Returns `Err` only when the frontmatter is
/// present but is not valid YAML.
///
/// Both the [`Agent`] (with body) and the [`CorpusEntry`] (index row with
/// the three split hashes) are returned together so a single parse feeds
/// both list/detail views and the on-disk index.
pub fn parse_agent(
    slug: &str,
    category: &str,
    source: &str,
) -> Result<Option<(Agent, CorpusEntry)>, String> {
    let Some(split) = split_frontmatter(source) else {
        return Ok(None);
    };

    let fm: Frontmatter = match serde_yaml::from_str(split.frontmatter) {
        Ok(frontmatter) => frontmatter,
        Err(strict_error) => parse_legacy_frontmatter(split.frontmatter)
            .map_err(|_| format!("{slug}: frontmatter YAML parse error: {strict_error}"))?,
    };

    // `name` is required; without it the file is not an agent.
    let Some(name) = fm.name.filter(|n| !n.trim().is_empty()) else {
        return Ok(None);
    };

    let description = fm.description.unwrap_or_default();
    let body = split.body.to_string();

    // Hash the canonical byte regions of the *source* (not a
    // re-serialization) so the values are stable across runs/platforms.
    let source_hash = sha256_hex(source.as_bytes());
    let frontmatter_hash = sha256_hex(split.frontmatter.as_bytes());
    let body_hash = sha256_hex(split.body.as_bytes());

    let agent = Agent {
        slug: slug.to_string(),
        name: name.clone(),
        description: description.clone(),
        localizations: BTreeMap::new(),
        category: category.to_string(),
        emoji: fm.emoji.clone(),
        color: fm.color.clone(),
        vibe: fm.vibe.clone(),
        body,
    };

    let entry = CorpusEntry {
        slug: slug.to_string(),
        name,
        category: category.to_string(),
        emoji: fm.emoji,
        color: fm.color,
        vibe: fm.vibe,
        description,
        source_hash,
        frontmatter_hash,
        body_hash,
    };

    Ok(Some((agent, entry)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: Frontend Developer\ndescription: \"Builds delightful UIs.\"\ncolor: blue\nemoji: 🎨\nvibe: Ships pixel-perfect interfaces.\n---\n# Frontend Developer Agent\n\nYou are a frontend developer.\n";
    const LEGACY_COLON_SAMPLE: &str = "---\nname: Developer Tooling Engineer\ndescription: Expert developer-tooling and CLI engineer — building command-line tools and internal developer platforms with great DX: intuitive command design, helpful errors, shell completions, fast startup, cross-platform distribution, and scriptable, composable interfaces.\ncolor: \"#4F46E5\"\nemoji: 🛠️\nvibe: The tool developers reach for is the one that respects their time.\n---\n# Developer Tooling Engineer\n\nCanonical body stays byte-for-byte unchanged.\n";

    #[test]
    fn parses_full_frontmatter() {
        let (agent, entry) = parse_agent("frontend-developer", "engineering", SAMPLE)
            .expect("ok")
            .expect("some");
        assert_eq!(agent.slug, "frontend-developer");
        assert_eq!(agent.name, "Frontend Developer");
        assert_eq!(agent.category, "engineering");
        assert_eq!(agent.description, "Builds delightful UIs.");
        assert_eq!(agent.color.as_deref(), Some("blue"));
        assert_eq!(agent.emoji.as_deref(), Some("🎨"));
        assert_eq!(
            agent.vibe.as_deref(),
            Some("Ships pixel-perfect interfaces.")
        );
        assert!(agent.body.starts_with("# Frontend Developer Agent"));
        // Index row mirrors the agent metadata.
        assert_eq!(entry.name, agent.name);
        assert_eq!(entry.description, agent.description);
    }

    #[test]
    fn hashes_are_deterministic_lowercase_hex_64() {
        let (_, e1) = parse_agent("x", "engineering", SAMPLE).unwrap().unwrap();
        let (_, e2) = parse_agent("x", "engineering", SAMPLE).unwrap().unwrap();
        // Stable across parses.
        assert_eq!(e1.source_hash, e2.source_hash);
        assert_eq!(e1.frontmatter_hash, e2.frontmatter_hash);
        assert_eq!(e1.body_hash, e2.body_hash);
        // 64 lowercase hex chars.
        for h in [&e1.source_hash, &e1.frontmatter_hash, &e1.body_hash] {
            assert_eq!(h.len(), 64);
            assert!(h
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
        // The three regions differ, so their hashes differ.
        assert_ne!(e1.frontmatter_hash, e1.body_hash);
        assert_ne!(e1.source_hash, e1.frontmatter_hash);
    }

    #[test]
    fn body_only_change_keeps_frontmatter_hash() {
        // Cosmetic vs substantive classification depends on this: a body
        // edit must move body_hash + source_hash but NOT frontmatter_hash.
        let edited = SAMPLE.replace(
            "You are a frontend developer.",
            "You are a senior frontend dev.",
        );
        let (_, base) = parse_agent("x", "engineering", SAMPLE).unwrap().unwrap();
        let (_, mutated) = parse_agent("x", "engineering", &edited).unwrap().unwrap();
        assert_eq!(base.frontmatter_hash, mutated.frontmatter_hash);
        assert_ne!(base.body_hash, mutated.body_hash);
        assert_ne!(base.source_hash, mutated.source_hash);
    }

    #[test]
    fn file_without_frontmatter_is_not_an_agent() {
        let md = "# Just a README\n\nNo frontmatter here.\n";
        assert!(parse_agent("readme", "examples", md).unwrap().is_none());
    }

    #[test]
    fn frontmatter_without_name_is_not_an_agent() {
        let md = "---\ndescription: orphan\n---\nbody\n";
        assert!(parse_agent("orphan", "examples", md).unwrap().is_none());
    }

    #[test]
    fn tolerates_leading_bom_and_trailing_fence_whitespace() {
        let md = "\u{feff}---  \nname: X\n---  \nbody\n";
        let (agent, _) = parse_agent("x", "c", md).unwrap().unwrap();
        assert_eq!(agent.name, "X");
        assert_eq!(agent.body, "body\n");
    }

    #[test]
    fn unclosed_frontmatter_is_not_an_agent() {
        let md = "---\nname: X\nstill in frontmatter\n";
        assert!(parse_agent("x", "c", md).unwrap().is_none());
    }

    #[test]
    fn missing_optional_fields_default_cleanly() {
        let md = "---\nname: Minimal\n---\nbody\n";
        let (agent, entry) = parse_agent("minimal", "c", md).unwrap().unwrap();
        assert_eq!(agent.description, "");
        assert!(agent.emoji.is_none());
        assert!(agent.color.is_none());
        assert!(agent.vibe.is_none());
        assert_eq!(entry.description, "");
    }

    #[test]
    fn legacy_plain_scalar_with_colon_space_preserves_metadata_body_and_hashes() {
        let (agent, entry) = parse_agent(
            "engineering-developer-tooling-engineer",
            "engineering",
            LEGACY_COLON_SAMPLE,
        )
        .expect("legacy canonical frontmatter should be compatible")
        .expect("agent should be indexed");

        let expected_description = "Expert developer-tooling and CLI engineer — building command-line tools and internal developer platforms with great DX: intuitive command design, helpful errors, shell completions, fast startup, cross-platform distribution, and scriptable, composable interfaces.";
        assert_eq!(agent.slug, "engineering-developer-tooling-engineer");
        assert_eq!(agent.name, "Developer Tooling Engineer");
        assert_eq!(agent.description, expected_description);
        assert_eq!(entry.description, expected_description);
        assert_eq!(agent.color.as_deref(), Some("#4F46E5"));
        assert_eq!(agent.emoji.as_deref(), Some("🛠️"));
        assert_eq!(
            agent.body,
            "# Developer Tooling Engineer\n\nCanonical body stays byte-for-byte unchanged.\n"
        );

        let split = split_frontmatter(LEGACY_COLON_SAMPLE).expect("valid fences");
        assert_eq!(
            entry.source_hash,
            sha256_hex(LEGACY_COLON_SAMPLE.as_bytes())
        );
        assert_eq!(
            entry.frontmatter_hash,
            sha256_hex(split.frontmatter.as_bytes())
        );
        assert_eq!(entry.body_hash, sha256_hex(split.body.as_bytes()));
    }

    #[test]
    fn quoted_and_multiline_yaml_still_use_strict_semantics() {
        let md = "---\nname: \"Quoted: Agent\"\ndescription: >-\n  First line with a colon: retained.\n  Second folded line.\ncolor: '#123456'\n---\nbody\n";
        let (agent, _) = parse_agent("quoted", "c", md).unwrap().unwrap();
        assert_eq!(agent.name, "Quoted: Agent");
        assert_eq!(
            agent.description,
            "First line with a colon: retained. Second folded line."
        );
        assert_eq!(agent.color.as_deref(), Some("#123456"));
    }

    #[test]
    fn legacy_fallback_preserves_valid_multiline_peer_fields() {
        let md = "---\nname: Legacy Agent\ndescription: Legacy description: colon retained.\nvibe: >-\n  First folded line.\n  Second folded line.\ncolor: \"#123456\"\n---\nbody\n";
        let (agent, _) = parse_agent("legacy", "c", md).unwrap().unwrap();
        assert_eq!(agent.description, "Legacy description: colon retained.");
        assert_eq!(
            agent.vibe.as_deref(),
            Some("First folded line. Second folded line.")
        );
        assert_eq!(agent.color.as_deref(), Some("#123456"));
    }

    #[test]
    fn unrelated_malformed_frontmatter_remains_an_error() {
        let md = "---\nname: Broken\ndescription: [not: closed\n---\nbody\n";
        assert!(parse_agent("broken", "c", md).is_err());
    }

    #[test]
    fn legacy_fallback_rejects_indicator_led_or_unknown_metadata() {
        let indicator = "---\nname: Broken\ndescription: - unsafe: mapping\n---\nbody\n";
        assert!(parse_agent("indicator", "c", indicator).is_err());

        let unknown = "---\nname: Broken\nunknown: unsafe: mapping\n---\nbody\n";
        assert!(parse_agent("unknown", "c", unknown).is_err());
    }

    #[tokio::test]
    async fn configured_catalog_has_273_agents_and_localizes_legacy_agent() {
        let Some(root) = std::env::var_os("AGENCY_AGENTS_TEST_CATALOG_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let categories = super::super::discover_categories(&root);
        let mut corpus = super::super::build_from_dir(&root, "hotfix-test", &categories)
            .await
            .expect("configured catalog should build");
        assert_eq!(corpus.count(), 273, "every canonical Agent must be indexed");

        let fallback_root = std::path::Path::new("resources/corpus-baseline");
        let localized = super::super::localization::apply(&root, fallback_root, &mut corpus.agents)
            .expect("approved zh-TW resource should apply");
        assert_eq!(localized, 273, "every canonical Agent must localize");

        let agent = corpus
            .get("engineering-developer-tooling-engineer")
            .expect("legacy Developer Tooling Engineer must be present");
        assert_eq!(agent.name, "Developer Tooling Engineer");
        assert!(agent
            .description
            .contains("great DX: intuitive command design"));
        let zh_tw = agent
            .localizations
            .get("zh-TW")
            .expect("legacy Agent must receive its existing zh-TW mapping");
        assert_eq!(zh_tw.name, "開發工具工程師");
        assert!(zh_tw.description.contains("命令列工具"));
    }
}
