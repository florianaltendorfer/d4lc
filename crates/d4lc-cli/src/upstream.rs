use crate::to_json_text;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

pub const UPSTREAM_CONFIG: &str = "config/upstream-files.toml";
pub const UPSTREAM_LOCK: &str = "upstream.lock";
pub const UPSTREAM_MANIFEST_PATH: &str = "licenses/upstream-file-manifest.json";
pub const UPSTREAM_PROVENANCE_PATH: &str = "licenses/license-provenance.json";
pub const ASSET_MANIFEST_PATH: &str = "assets/provenance/asset-manifest.json";

const LICENSE_PATH: &str = "LICENSE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamConfig {
    pub url: String,
    pub branch: String,
    pub local_path: String,
    pub patterns: Vec<TrackedPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedPattern {
    pub category: String,
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamLock {
    pub last_reviewed_commit: String,
    pub last_reviewed_release: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestItem {
    pub category: String,
    pub source_path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedPattern {
    pub category: String,
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamManifest {
    pub config: UpstreamConfig,
    pub upstream_lock: UpstreamLock,
    pub items: Vec<ManifestItem>,
    pub unresolved_patterns: Vec<UnresolvedPattern>,
}

pub fn upstream_report(root: &Path) -> Result<String, String> {
    let manifest = build_manifest(root)?;

    let mut output = String::new();
    output.push_str("UPSTREAM_REPORT\n");
    append_presence(
        &mut output,
        "upstream_root",
        &root.join(&manifest.config.local_path),
    );
    append_presence(&mut output, "manifest_config", &root.join(UPSTREAM_CONFIG));
    append_presence(
        &mut output,
        "manifest_json",
        &root.join(UPSTREAM_MANIFEST_PATH),
    );
    append_presence(
        &mut output,
        "provenance_json",
        &root.join(UPSTREAM_PROVENANCE_PATH),
    );
    output.push_str(&format!("upstream_url={}\n", manifest.config.url));
    output.push_str(&format!("upstream_branch={}\n", manifest.config.branch));
    output.push_str(&format!(
        "last_reviewed_commit={}\n",
        manifest.upstream_lock.last_reviewed_commit
    ));

    let mut services = 0usize;
    let mut entities = 0usize;
    let mut data = 0usize;
    let mut presets = 0usize;
    let mut wiki = 0usize;
    let mut ui = 0usize;

    for item in &manifest.items {
        match item.category.as_str() {
            "services" => services += 1,
            "entities" => entities += 1,
            "data_globs" => data += 1,
            "preset_globs" => presets += 1,
            "wiki_globs" => wiki += 1,
            "ui_globs" => ui += 1,
            _ => {}
        }
        output.push_str(&format!(
            "tracked_file category={} path={} sha256={} bytes={}\n",
            item.category, item.source_path, item.sha256, item.bytes
        ));
    }

    for unresolved in &manifest.unresolved_patterns {
        output.push_str(&format!(
            "unresolved_pattern category={} pattern={} reason={}\n",
            unresolved.category, unresolved.pattern, unresolved.reason
        ));
    }

    output.push_str(&format!("tracked_files={}\n", manifest.items.len()));
    output.push_str(&format!(
        "unresolved_patterns={}\n",
        manifest.unresolved_patterns.len()
    ));
    output.push_str(&format!("category_services={services}\n"));
    output.push_str(&format!("category_entities={entities}\n"));
    output.push_str(&format!("category_data={data}\n"));
    output.push_str(&format!("category_presets={presets}\n"));
    output.push_str(&format!("category_wiki={wiki}\n"));
    output.push_str(&format!("category_ui={ui}\n"));
    output.push_str("behavior_parity=not_evaluated\n");
    Ok(output)
}

pub fn upstream_manifest_command(root: &Path, write_path: Option<&Path>) -> Result<String, String> {
    let manifest = build_manifest(root)?;
    let json = manifest_json(&manifest);

    if let Some(path) = write_path {
        let full_path = root.join(path);
        write_file(&full_path, &json)?;
        Ok(format!(
            "UPSTREAM_MANIFEST_WRITTEN\npath={}\nitems={}\nunresolved_patterns={}\n",
            path.display(),
            manifest.items.len(),
            manifest.unresolved_patterns.len()
        ))
    } else {
        Ok(json)
    }
}

pub fn upstream_provenance_command(
    root: &Path,
    write_path: Option<&Path>,
) -> Result<String, String> {
    let manifest = build_manifest(root)?;
    let copied_files = read_asset_manifest_items_if_present(root)?;
    let license_full_path = root.join(&manifest.config.local_path).join(LICENSE_PATH);
    let (license_sha256, license_bytes, license_status) = if license_full_path.is_file() {
        let bytes = read_bytes(&license_full_path)?;
        (sha256_hex(&bytes), bytes.len() as u64, "present")
    } else {
        ("UNKNOWN".to_string(), 0, "missing")
    };

    let json = provenance_json(
        &manifest.config,
        &manifest.upstream_lock,
        &license_sha256,
        license_bytes,
        license_status,
        &copied_files,
    );

    if let Some(path) = write_path {
        let full_path = root.join(path);
        write_file(&full_path, &json)?;
        Ok(format!(
            "UPSTREAM_PROVENANCE_WRITTEN\npath={}\nlicense_status={}\ncopied_files={}\n",
            path.display(),
            license_status,
            copied_files.len()
        ))
    } else {
        Ok(json)
    }
}

pub fn upstream_changed_command(root: &Path, remote_head: &str) -> Result<String, String> {
    validate_commitish("remote_head", remote_head)?;
    let upstream_lock = read_upstream_lock(root)?;
    let changed = remote_head != upstream_lock.last_reviewed_commit;

    Ok(format!(
        "UPSTREAM_CHANGE_REPORT\nremote_head={}\nlast_reviewed_commit={}\nUPSTREAM_CHANGED={}\n",
        remote_head, upstream_lock.last_reviewed_commit, changed
    ))
}

pub fn upstream_asset_parity_command(root: &Path) -> Result<String, String> {
    let manifest_path = require_developer_checkout(root, UPSTREAM_MANIFEST_PATH)?;
    let provenance_path = require_developer_checkout(root, UPSTREAM_PROVENANCE_PATH)?;
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let provenance_text = fs::read_to_string(&provenance_path)
        .map_err(|error| format!("failed to read {}: {error}", provenance_path.display()))?;

    let manifest = parse_manifest_items(&manifest_text)?;
    let copied_files = parse_provenance_copied_files(&provenance_text)?;
    if copied_files.is_empty() {
        return Err(
            "UPSTREAM_ASSET_PARITY\nerror=no copied_files entries found in licenses/license-provenance.json\nhint=refresh reviewed provenance artifacts before running this gate\n".to_string()
        );
    }

    let mut expected_by_destination = BTreeMap::new();
    let mut root_dirs = BTreeSet::new();
    for copied in &copied_files {
        expected_by_destination.insert(copied.destination_path.clone(), copied.clone());
        if let Some(parent) = Path::new(&copied.destination_path).parent() {
            root_dirs.insert(normalize_path(&parent.to_string_lossy()));
        }
    }

    let mut actual_paths = BTreeSet::new();
    for root_dir in root_dirs {
        let full_root = root.join(&root_dir);
        if !full_root.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_files(&full_root, &mut files)?;
        for file in files {
            let relative = file
                .strip_prefix(root)
                .map_err(|error| format!("failed to relativize {}: {error}", file.display()))?;
            actual_paths.insert(normalize_path(&relative.to_string_lossy()));
        }
    }

    let mut missing = Vec::new();
    let mut changed = Vec::new();
    let mut manifest_mismatches = Vec::new();
    let mut upstream_config: Option<UpstreamConfig> = None;
    for copied in &copied_files {
        if let Some((archive_path, entry_path)) = copied.source_path.split_once("::") {
            if !manifest.contains_key(archive_path) {
                manifest_mismatches.push(format!(
                    "{} source={} expected_manifest_archive={archive_path} status=MISSING",
                    copied.destination_path, copied.source_path
                ));
            } else {
                if upstream_config.is_none() {
                    upstream_config = Some(read_upstream_config(root)?);
                }
                let config = upstream_config
                    .as_ref()
                    .ok_or_else(|| "failed to load upstream config".to_string())?;
                let archive_full_path = require_developer_checkout(
                    root,
                    Path::new(&config.local_path).join(archive_path),
                )?;
                if !zip_archive_has_member(&archive_full_path, entry_path)? {
                    manifest_mismatches.push(format!(
                        "{} source={} archive_member={} status=MISSING",
                        copied.destination_path, copied.source_path, entry_path
                    ));
                }
            }
        } else if let Some(manifest_sha) = manifest.get(&copied.source_path) {
            if manifest_sha != &copied.sha256 {
                manifest_mismatches.push(format!(
                    "{} source={} expected_manifest_sha256={} provenance_sha256={}",
                    copied.destination_path, copied.source_path, manifest_sha, copied.sha256
                ));
            }
        } else {
            manifest_mismatches.push(format!(
                "{} source={} expected_manifest_sha256=MISSING provenance_sha256={}",
                copied.destination_path, copied.source_path, copied.sha256
            ));
        }

        let full_destination = root.join(&copied.destination_path);
        if !full_destination.is_file() {
            missing.push(copied.destination_path.clone());
            continue;
        }

        let bytes = read_bytes(&full_destination)?;
        let actual_sha = sha256_hex(&bytes);
        if actual_sha != copied.sha256 {
            changed.push(format!(
                "{} expected_sha256={} actual_sha256={}",
                copied.destination_path, copied.sha256, actual_sha
            ));
        }
    }

    let mut extra = Vec::new();
    for path in actual_paths {
        if !expected_by_destination.contains_key(&path) {
            extra.push(path);
        }
    }

    let mut baseline_failures = Vec::new();
    for (file_name, expected_count) in baseline_enus_counts() {
        let Some(copied) = copied_files
            .iter()
            .find(|item| item.destination_path.ends_with(file_name))
        else {
            baseline_failures.push(format!(
                "{file_name} missing from copied_files provenance entries"
            ));
            continue;
        };
        let full_path = root.join(&copied.destination_path);
        if !full_path.is_file() {
            baseline_failures.push(format!("{file_name} destination file missing"));
            continue;
        }
        let actual_count = count_top_level_json_array_entries(&full_path)?;
        if actual_count != expected_count {
            baseline_failures.push(format!(
                "{file_name} expected_count={expected_count} actual_count={actual_count}"
            ));
        }
    }

    let ok = missing.is_empty()
        && extra.is_empty()
        && changed.is_empty()
        && manifest_mismatches.is_empty()
        && baseline_failures.is_empty();

    let mut output = String::new();
    output.push_str("UPSTREAM_ASSET_PARITY\n");
    output.push_str(&format!("manifest_path={}\n", UPSTREAM_MANIFEST_PATH));
    output.push_str(&format!("provenance_path={}\n", UPSTREAM_PROVENANCE_PATH));
    output.push_str(&format!("copied_files_expected={}\n", copied_files.len()));
    output.push_str(&format!("missing_files={}\n", missing.len()));
    output.push_str(&format!("extra_files={}\n", extra.len()));
    output.push_str(&format!("changed_files={}\n", changed.len()));
    output.push_str(&format!(
        "manifest_mismatches={}\n",
        manifest_mismatches.len()
    ));
    output.push_str(&format!(
        "baseline_enus_failures={}\n",
        baseline_failures.len()
    ));
    for item in &missing {
        output.push_str(&format!("missing path={item}\n"));
    }
    for item in &extra {
        output.push_str(&format!("extra path={item}\n"));
    }
    for item in &changed {
        output.push_str(&format!("changed {item}\n"));
    }
    for item in &manifest_mismatches {
        output.push_str(&format!("manifest_mismatch {item}\n"));
    }
    for item in &baseline_failures {
        output.push_str(&format!("baseline_failure {item}\n"));
    }
    output.push_str(&format!("UPSTREAM_ASSET_PARITY_OK={ok}\n"));

    if ok {
        Ok(output)
    } else {
        Err(output)
    }
}

pub fn build_manifest(root: &Path) -> Result<UpstreamManifest, String> {
    require_developer_checkout(root, UPSTREAM_CONFIG)?;
    let config = read_upstream_config(root)?;
    let upstream_lock = read_upstream_lock(root)?;
    // Without the source tree every pattern would be reported unresolved,
    // which reads like an upstream change instead of a missing checkout.
    let upstream_root = require_developer_checkout(root, &config.local_path)?;

    let mut items = Vec::new();
    let mut unresolved_patterns = Vec::new();

    for tracked in &config.patterns {
        let resolved_paths = resolve_pattern(&upstream_root, &tracked.pattern)?;
        if resolved_paths.is_empty() {
            unresolved_patterns.push(UnresolvedPattern {
                category: tracked.category.clone(),
                pattern: tracked.pattern.clone(),
                reason: "no_matches".to_string(),
            });
            continue;
        }

        for source_path in resolved_paths {
            let full_path = upstream_root.join(&source_path);
            let bytes = read_bytes(&full_path)?;
            items.push(ManifestItem {
                category: tracked.category.clone(),
                source_path,
                sha256: sha256_hex(&bytes),
                bytes: bytes.len() as u64,
            });
        }
    }

    items.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then(left.source_path.cmp(&right.source_path))
    });
    unresolved_patterns.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then(left.pattern.cmp(&right.pattern))
    });

    Ok(UpstreamManifest {
        config,
        upstream_lock,
        items,
        unresolved_patterns,
    })
}

/// `root/relative`, or an error naming it: the upstream source tree and the
/// `config/` and `licenses/` metadata exist in a developer checkout only;
/// the release packages do not ship them.
fn require_developer_checkout(root: &Path, relative: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = root.join(relative);
    if path.exists() {
        return Ok(path);
    }
    let shown = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
    Err(format!(
        "this command requires a developer checkout: {} is missing",
        shown.display()
    ))
}

pub fn read_upstream_config(root: &Path) -> Result<UpstreamConfig, String> {
    let path = root.join(UPSTREAM_CONFIG);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_upstream_config(&text)
}

pub fn read_upstream_lock(root: &Path) -> Result<UpstreamLock, String> {
    let path = root.join(UPSTREAM_LOCK);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(UpstreamLock {
        last_reviewed_commit: parse_key_value(&text, "last_reviewed_commit")
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        last_reviewed_release: parse_key_value(&text, "last_reviewed_release")
            .unwrap_or_else(|| "UNKNOWN".to_string()),
    })
}

pub fn parse_upstream_config(text: &str) -> Result<UpstreamConfig, String> {
    let mut section = String::new();
    let mut active_array: Option<String> = None;
    let mut url = None;
    let mut branch = None;
    let mut local_path = None;
    let mut patterns = Vec::new();

    for raw_line in text.lines() {
        let line_without_comment = raw_line.split('#').next().unwrap_or("");
        let trimmed = line_without_comment.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.contains('=') {
            section = trimmed
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            active_array = None;
            continue;
        }

        if section == "upstream" {
            if let Some(value) = parse_assignment_value(trimmed, "url") {
                url = Some(value);
            } else if let Some(value) = parse_assignment_value(trimmed, "branch") {
                branch = Some(value);
            } else if let Some(value) = parse_assignment_value(trimmed, "local_path") {
                local_path = Some(value);
            }
            continue;
        }

        if section != "critical_files" {
            continue;
        }

        if let Some(array_key) = active_array.clone() {
            for value in extract_quoted_values(trimmed) {
                patterns.push(TrackedPattern {
                    category: array_key.clone(),
                    pattern: value,
                });
            }
            if trimmed.contains(']') {
                active_array = None;
            }
            continue;
        }

        if let Some((key, value_part)) = parse_array_start(trimmed) {
            for value in extract_quoted_values(value_part) {
                patterns.push(TrackedPattern {
                    category: key.clone(),
                    pattern: value,
                });
            }
            if !value_part.contains(']') {
                active_array = Some(key);
            }
        }
    }

    let config = UpstreamConfig {
        url: url
            .ok_or_else(|| "missing [upstream].url in config/upstream-files.toml".to_string())?,
        branch: branch
            .ok_or_else(|| "missing [upstream].branch in config/upstream-files.toml".to_string())?,
        local_path: local_path.ok_or_else(|| {
            "missing [upstream].local_path in config/upstream-files.toml".to_string()
        })?,
        patterns,
    };

    if config.patterns.is_empty() {
        return Err("no [critical_files] patterns found in config/upstream-files.toml".to_string());
    }

    Ok(config)
}

pub fn parse_key_value(text: &str, key: &str) -> Option<String> {
    for raw_line in text.lines() {
        let line_without_comment = raw_line.split('#').next().unwrap_or("");
        let trimmed = line_without_comment.trim();
        if let Some(value) = parse_assignment_value(trimmed, key) {
            return Some(value);
        }
    }
    None
}

pub fn resolve_pattern(upstream_root: &Path, pattern: &str) -> Result<Vec<String>, String> {
    if !pattern.contains('*') {
        let full_path = upstream_root.join(pattern);
        return if full_path.is_file() {
            Ok(vec![normalize_path(pattern)])
        } else {
            Ok(Vec::new())
        };
    }

    let base = base_path_before_wildcard(pattern);
    let search_root = upstream_root.join(&base);
    if !search_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_files(&search_root, &mut files)?;

    let mut matches = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(upstream_root)
            .map_err(|error| format!("failed to relativize {}: {error}", file.display()))?;
        let relative_text = normalize_path(&relative.to_string_lossy());
        if pattern_matches(pattern, &relative_text) {
            matches.push(relative_text);
        }
    }
    matches.sort();
    Ok(matches)
}

pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern_segments = split_path(pattern);
    let path_segments = split_path(path);
    segments_match(&pattern_segments, &path_segments)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn append_presence(output: &mut String, label: &str, path: &Path) {
    let status = if path.exists() { "present" } else { "missing" };
    output.push_str(&format!("{label}={} {status}\n", path.display()));
}

fn parse_assignment_value(line: &str, key: &str) -> Option<String> {
    let (left, right) = line.split_once('=')?;
    if left.trim() != key {
        return None;
    }
    extract_quoted_values(right).into_iter().next()
}

fn parse_array_start(line: &str) -> Option<(String, &str)> {
    let (left, right) = line.split_once('=')?;
    if !right.contains('[') {
        return None;
    }
    Some((left.trim().to_string(), right))
}

pub(crate) fn extract_quoted_values(line: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = line;

    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            break;
        };
        values.push(after_start[..end].to_string());
        rest = &after_start[end + 1..];
    }

    values
}

fn base_path_before_wildcard(pattern: &str) -> PathBuf {
    let mut base = PathBuf::new();
    for segment in split_path(pattern) {
        if segment.contains('*') {
            break;
        }
        base.push(segment);
    }
    base
}

fn split_path(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn segments_match(pattern_segments: &[&str], path_segments: &[&str]) -> bool {
    if pattern_segments.is_empty() {
        return path_segments.is_empty();
    }

    if pattern_segments[0] == "**" {
        return segments_match(&pattern_segments[1..], path_segments)
            || (!path_segments.is_empty()
                && segments_match(pattern_segments, &path_segments[1..]));
    }

    if path_segments.is_empty() {
        return false;
    }

    segment_matches(pattern_segments[0], path_segments[0])
        && segments_match(&pattern_segments[1..], &path_segments[1..])
}

fn segment_matches(pattern: &str, text: &str) -> bool {
    let pattern_bytes = pattern.as_bytes();
    let text_bytes = text.as_bytes();
    let mut pattern_index = 0usize;
    let mut text_index = 0usize;
    let mut star_index = None;
    let mut match_index = 0usize;

    while text_index < text_bytes.len() {
        if pattern_index < pattern_bytes.len()
            && pattern_bytes[pattern_index] == text_bytes[text_index]
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern_bytes.len() && pattern_bytes[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            match_index = text_index;
            pattern_index += 1;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            match_index += 1;
            text_index = match_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern_bytes.len() && pattern_bytes[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern_bytes.len()
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read directory {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, text).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProvenanceCopiedFile {
    destination_path: String,
    source_path: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetManifestItem {
    destination_path: String,
    source_path: String,
    upstream_commit: String,
    sha256: String,
    license: String,
}

fn parse_manifest_items(text: &str) -> Result<BTreeMap<String, String>, String> {
    let objects = json_array_of_objects(text, "items")?;
    let mut items = BTreeMap::new();
    for object in objects {
        let source_path = string_field(&object, "source_path")
            .ok_or_else(|| "manifest item missing source_path".to_string())?;
        let sha256 = string_field(&object, "sha256")
            .ok_or_else(|| format!("manifest item missing sha256 for source_path={source_path}"))?;
        items.insert(source_path, sha256);
    }
    Ok(items)
}

fn parse_provenance_copied_files(text: &str) -> Result<Vec<ProvenanceCopiedFile>, String> {
    let objects = json_array_of_objects(text, "copied_files")?;
    let mut files = Vec::new();
    for object in objects {
        let destination_path = string_field(&object, "destination_path")
            .ok_or_else(|| "copied file missing destination_path".to_string())?;
        let source_path = string_field(&object, "source_path")
            .ok_or_else(|| format!("copied file missing source_path for {destination_path}"))?;
        let sha256 = string_field(&object, "sha256")
            .ok_or_else(|| format!("copied file missing sha256 for {destination_path}"))?;
        files.push(ProvenanceCopiedFile {
            destination_path: normalize_path(&destination_path),
            source_path: normalize_path(&source_path),
            sha256,
        });
    }
    files.sort_by(|left, right| left.destination_path.cmp(&right.destination_path));
    Ok(files)
}

fn read_asset_manifest_items_if_present(root: &Path) -> Result<Vec<AssetManifestItem>, String> {
    let asset_manifest_path = root.join(ASSET_MANIFEST_PATH);
    if !asset_manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let asset_manifest_text = fs::read_to_string(&asset_manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", asset_manifest_path.display()))?;
    parse_asset_manifest_items(&asset_manifest_text)
}

fn parse_asset_manifest_items(text: &str) -> Result<Vec<AssetManifestItem>, String> {
    let objects = json_array_of_objects(text, "items")?;
    let mut items = Vec::new();
    for object in objects {
        let destination_path = string_field(&object, "destination_path")
            .ok_or_else(|| "asset manifest item missing destination_path".to_string())?;
        let source_path = string_field(&object, "source_path").ok_or_else(|| {
            format!("asset manifest item missing source_path for {destination_path}")
        })?;
        let upstream_commit = string_field(&object, "upstream_commit").ok_or_else(|| {
            format!("asset manifest item missing upstream_commit for {destination_path}")
        })?;
        let sha256 = string_field(&object, "sha256")
            .ok_or_else(|| format!("asset manifest item missing sha256 for {destination_path}"))?;
        let license = string_field(&object, "license")
            .ok_or_else(|| format!("asset manifest item missing license for {destination_path}"))?;
        items.push(AssetManifestItem {
            destination_path: normalize_path(&destination_path),
            source_path: normalize_path(&source_path),
            upstream_commit,
            sha256,
            license,
        });
    }
    items.sort_by(|left, right| left.destination_path.cmp(&right.destination_path));
    Ok(items)
}

fn zip_archive_has_member(archive_path: &Path, member_path: &str) -> Result<bool, String> {
    use zip::result::ZipError;

    let file = fs::File::open(archive_path)
        .map_err(|error| format!("failed to open archive {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|error| format!("failed to read archive {}: {error}", archive_path.display()))?;
    let member_path = normalize_path(member_path);
    let result = match archive.by_name(&member_path) {
        Ok(_) => Ok(true),
        Err(ZipError::FileNotFound) => Ok(false),
        Err(error) => Err(format!(
            "failed to read member {} from {}: {error}",
            member_path,
            archive_path.display()
        )),
    };
    result
}
fn baseline_enus_counts() -> [(&'static str, usize); 8] {
    [
        ("Affixes.enUS.json", 894),
        ("Aspects.enUS.json", 523),
        ("ItemTypes.enUS.json", 432),
        ("ParagonBoards.enUS.json", 80),
        ("ParagonGlyphs.enUS.json", 161),
        ("Runes.enUS.json", 52),
        ("Sigils.enUS.json", 340),
        ("Uniques.enUS.json", 308),
    ]
}
/// The objects of the top-level array field `key`, as string maps.
fn json_array_of_objects(
    text: &str,
    key: &str,
) -> Result<Vec<serde_json::Map<String, Value>>, String> {
    let document: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid json: {error}"))?;
    let items = document
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key} array in json"))?;
    Ok(items
        .iter()
        .filter_map(|item| item.as_object().cloned())
        .collect())
}

fn string_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

fn count_top_level_json_array_entries(path: &Path) -> Result<usize, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str::<Vec<Value>>(&text)
        .map(|items| items.len())
        .map_err(|error| {
            format!(
                "expected top-level JSON array in {}: {error}",
                path.display()
            )
        })
}

fn validate_commitish(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{label} must not contain whitespace"));
    }
    Ok(())
}

fn upstream_json(config: &UpstreamConfig, upstream_lock: &UpstreamLock) -> Value {
    json!({
        "url": config.url,
        "branch": config.branch,
        "local_path": config.local_path,
        "last_reviewed_commit": upstream_lock.last_reviewed_commit,
        "last_reviewed_release": upstream_lock.last_reviewed_release,
    })
}

fn manifest_json(manifest: &UpstreamManifest) -> String {
    to_json_text(&json!({
        "schema_version": 1,
        "upstream": upstream_json(&manifest.config, &manifest.upstream_lock),
        "source_config": "config/upstream-files.toml",
        "items": manifest
            .items
            .iter()
            .map(|item| json!({
                "category": item.category,
                "source_path": item.source_path,
                "sha256": item.sha256,
                "bytes": item.bytes,
            }))
            .collect::<Vec<_>>(),
        "unresolved_patterns": manifest
            .unresolved_patterns
            .iter()
            .map(|item| json!({
                "category": item.category,
                "pattern": item.pattern,
                "reason": item.reason,
            }))
            .collect::<Vec<_>>(),
    }))
}

fn provenance_json(
    config: &UpstreamConfig,
    upstream_lock: &UpstreamLock,
    license_sha256: &str,
    license_bytes: u64,
    license_status: &str,
    copied_files: &[AssetManifestItem],
) -> String {
    to_json_text(&json!({
        "schema_version": 2,
        "upstream": upstream_json(config, upstream_lock),
        "license": {
            "spdx": "MIT",
            "source_path": "upstream/Diablo4Companion/LICENSE",
            "sha256": license_sha256,
            "bytes": license_bytes,
            "status": license_status,
        },
        "copied_files": copied_files
            .iter()
            .map(|item| json!({
                "destination_path": item.destination_path,
                "source_path": item.source_path,
                "upstream_commit": item.upstream_commit,
                "sha256": item.sha256,
                "license": item.license,
            }))
            .collect::<Vec<_>>(),
        "notes": [
            "Copied file provenance is synchronized from assets/provenance/asset-manifest.json."
        ],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_upstream_config_with_separate_categories() {
        let config = parse_upstream_config(
            r#"
[upstream]
url = "https://example.invalid/repo.git"
branch = "master"
local_path = "upstream/example"

[critical_files]
services = [
  "A.cs",
]
preset_globs = [
  "downloads/systempresets/*.zip",
]
"#,
        )
        .unwrap_or_else(|error| panic!("unexpected parse error: {error}"));

        assert_eq!(config.url, "https://example.invalid/repo.git");
        assert_eq!(config.branch, "master");
        assert_eq!(config.local_path, "upstream/example");
        assert_eq!(
            config.patterns,
            vec![
                TrackedPattern {
                    category: "services".to_string(),
                    pattern: "A.cs".to_string(),
                },
                TrackedPattern {
                    category: "preset_globs".to_string(),
                    pattern: "downloads/systempresets/*.zip".to_string(),
                }
            ]
        );
    }

    #[test]
    fn matches_single_and_recursive_globs() {
        assert!(pattern_matches(
            "D4Companion/Data/*.json",
            "D4Companion/Data/A.json"
        ));
        assert!(!pattern_matches(
            "D4Companion/Data/*.json",
            "D4Companion/Data/Nested/A.json"
        ));
        assert!(pattern_matches(
            "D4Companion/ViewModels/**/*.cs",
            "D4Companion/ViewModels/Dialogs/A.cs"
        ));
        assert!(pattern_matches(
            "D4Companion/ViewModels/**/*.cs",
            "D4Companion/ViewModels/A.cs"
        ));
    }

    #[test]
    fn resolves_patterns_and_hashes_files() {
        let root = create_temp_repo("resolves_patterns_and_hashes_files");
        let upstream_root = root.join("upstream/original");
        create_file(&upstream_root.join("D4Companion/Data/A.json"), "{}\n");
        create_file(&upstream_root.join("D4Companion/Data/B.txt"), "nope\n");
        create_file(
            &upstream_root.join("D4Companion/ViewModels/Dialogs/A.cs"),
            "class A {}\n",
        );

        let json_matches = resolve_pattern(&upstream_root, "D4Companion/Data/*.json")
            .unwrap_or_else(|error| panic!("resolve failed: {error}"));
        let recursive_matches = resolve_pattern(&upstream_root, "D4Companion/ViewModels/**/*.cs")
            .unwrap_or_else(|error| panic!("resolve failed: {error}"));

        assert_eq!(json_matches, vec!["D4Companion/Data/A.json"]);
        assert_eq!(
            recursive_matches,
            vec!["D4Companion/ViewModels/Dialogs/A.cs"]
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        remove_dir(&root);
    }

    #[test]
    fn builds_manifest_with_unresolved_patterns() {
        let root = create_temp_repo("builds_manifest_with_unresolved_patterns");
        create_file(
            &root.join(UPSTREAM_CONFIG),
            r#"
[upstream]
url = "https://example.invalid/repo.git"
branch = "master"
local_path = "upstream/original"
[critical_files]
services = ["Service.cs"]
preset_globs = ["downloads/systempresets/*.zip"]
"#,
        );
        create_file(
            &root.join(UPSTREAM_LOCK),
            "last_reviewed_commit = \"UNKNOWN\"\nlast_reviewed_release = \"UNKNOWN\"\n",
        );
        create_file(&root.join("upstream/original/Service.cs"), "service\n");

        let manifest =
            build_manifest(&root).unwrap_or_else(|error| panic!("manifest build failed: {error}"));

        assert_eq!(manifest.items.len(), 1);
        assert_eq!(manifest.items[0].source_path, "Service.cs");
        assert_eq!(manifest.unresolved_patterns.len(), 1);
        assert_eq!(manifest.unresolved_patterns[0].category, "preset_globs");

        remove_dir(&root);
    }

    #[test]
    fn changed_command_reports_false_when_heads_match() {
        let root = create_temp_repo("changed_command_reports_false_when_heads_match");
        create_file(
            &root.join(UPSTREAM_LOCK),
            "last_reviewed_commit = \"abc123\"\nlast_reviewed_release = \"UNKNOWN\"\n",
        );

        let report = upstream_changed_command(&root, "abc123")
            .unwrap_or_else(|error| panic!("changed command failed: {error}"));

        assert!(report.contains("UPSTREAM_CHANGED=false"));
        remove_dir(&root);
    }

    #[test]
    fn upstream_provenance_command_includes_asset_manifest_copied_files() {
        let root =
            create_temp_repo("upstream_provenance_command_includes_asset_manifest_copied_files");
        create_file(
            &root.join(UPSTREAM_CONFIG),
            r#"
[upstream]
url = "https://example.invalid/repo.git"
branch = "master"
local_path = "upstream/original"
[critical_files]
data_globs = ["D4Companion/Data/*.json"]
"#,
        );
        create_file(
            &root.join(UPSTREAM_LOCK),
            "last_reviewed_commit = \"abc123\"\nlast_reviewed_release = \"v1.2.3\"\n",
        );
        create_file(&root.join("upstream/original/LICENSE"), "mit\n");
        create_file(
            &root.join("upstream/original/D4Companion/Data/Affixes.enUS.json"),
            "[]\n",
        );
        create_file(
            &root.join(ASSET_MANIFEST_PATH),
            r#"{
  "schema_version": 2,
  "upstream": {
    "url": "https://example.invalid/repo.git",
    "branch": "master",
    "last_reviewed_commit": "abc123",
    "last_reviewed_release": "v1.2.3"
  },
  "items": [
    {
      "destination_path": "assets/original/data/Affixes.enUS.json",
      "source_path": "D4Companion/Data/Affixes.enUS.json",
      "upstream_commit": "abc123",
      "sha256": "4f53cda18c2baa0c0354bb5f9a3ecbe5ed3c0a4d79d5400f0fdf7f60f7e2a6f3",
      "license": "MIT"
    }
  ]
}
"#,
        );

        let output = upstream_provenance_command(&root, None)
            .unwrap_or_else(|error| panic!("unexpected provenance error: {error}"));

        assert!(output.contains("\"schema_version\": 2"));
        assert!(output.contains("\"copied_files\": ["));
        assert!(output.contains("\"destination_path\": \"assets/original/data/Affixes.enUS.json\""));
        assert!(output.contains("\"upstream_commit\": \"abc123\""));
        remove_dir(&root);
    }

    #[test]
    fn upstream_provenance_command_allows_missing_asset_manifest() {
        let root = create_temp_repo("upstream_provenance_command_allows_missing_asset_manifest");
        create_file(
            &root.join(UPSTREAM_CONFIG),
            r#"
[upstream]
url = "https://example.invalid/repo.git"
branch = "master"
local_path = "upstream/original"
[critical_files]
services = ["Service.cs"]
"#,
        );
        create_file(
            &root.join(UPSTREAM_LOCK),
            "last_reviewed_commit = \"abc123\"\nlast_reviewed_release = \"UNKNOWN\"\n",
        );
        create_file(&root.join("upstream/original/LICENSE"), "mit\n");
        create_file(&root.join("upstream/original/Service.cs"), "service\n");

        let output = upstream_provenance_command(&root, None)
            .unwrap_or_else(|error| panic!("unexpected provenance error: {error}"));

        assert!(output.contains("\"schema_version\": 2"));
        assert!(output.contains("\"copied_files\": []"));
        remove_dir(&root);
    }

    #[test]
    fn upstream_asset_parity_reports_missing_extra_and_changed_files() {
        let root =
            create_temp_repo("upstream_asset_parity_reports_missing_extra_and_changed_files");
        create_file(&root.join("assets/original/data/Affixes.enUS.json"), "[]\n");
        create_file(
            &root.join("assets/original/data/Aspects.enUS.json"),
            "[{\"id\":1}]\n",
        );
        create_file(&root.join("assets/original/data/extra.txt"), "extra\n");

        let affixes_sha = file_sha(&root.join("assets/original/data/Affixes.enUS.json"));
        create_file(
            &root.join(UPSTREAM_MANIFEST_PATH),
            &format!(
                "{{\"items\":[{{\"source_path\":\"D4Companion/Data/Affixes.enUS.json\",\"sha256\":\"{}\"}},{{\"source_path\":\"D4Companion/Data/Aspects.enUS.json\",\"sha256\":\"{}\"}}]}}\n",
                affixes_sha, "93f6de69f880f1f7f7bfecb22ebbd7f80ac52c19f5f537f6ae846b79554f7f80"
            ),
        );
        create_file(
            &root.join(UPSTREAM_PROVENANCE_PATH),
            &format!(
                "{{\"copied_files\":[{{\"destination_path\":\"assets/original/data/Affixes.enUS.json\",\"source_path\":\"D4Companion/Data/Affixes.enUS.json\",\"sha256\":\"{}\"}},{{\"destination_path\":\"assets/original/data/Aspects.enUS.json\",\"source_path\":\"D4Companion/Data/Aspects.enUS.json\",\"sha256\":\"{}\"}}]}}\n",
                affixes_sha, "93f6de69f880f1f7f7bfecb22ebbd7f80ac52c19f5f537f6ae846b79554f7f80"
            ),
        );

        let result = match upstream_asset_parity_command(&root) {
            Ok(output) => panic!("parity should fail, got success output: {output}"),
            Err(output) => output,
        };
        assert!(result.contains("missing_files=0"));
        assert!(result.contains("extra_files=1"));
        assert!(result.contains("changed_files=1"));
        assert!(result.contains("extra path=assets/original/data/extra.txt"));
        assert!(result.contains("changed assets/original/data/Aspects.enUS.json"));
        assert!(result.contains("baseline_enus_failures="));
        remove_dir(&root);
    }

    #[test]
    fn upstream_asset_parity_fails_on_changed_hash_fixture() {
        let root = create_temp_repo("upstream_asset_parity_fails_on_changed_hash_fixture");
        write_array_file(&root, "assets/original/data/Affixes.enUS.json", 894);
        write_array_file(&root, "assets/original/data/Aspects.enUS.json", 523);
        write_array_file(&root, "assets/original/data/ItemTypes.enUS.json", 432);
        write_array_file(&root, "assets/original/data/ParagonBoards.enUS.json", 80);
        write_array_file(&root, "assets/original/data/ParagonGlyphs.enUS.json", 161);
        write_array_file(&root, "assets/original/data/Runes.enUS.json", 52);
        write_array_file(&root, "assets/original/data/Sigils.enUS.json", 340);
        write_array_file(&root, "assets/original/data/Uniques.enUS.json", 308);

        let entries = [
            (
                "assets/original/data/Affixes.enUS.json",
                "D4Companion/Data/Affixes.enUS.json",
            ),
            (
                "assets/original/data/Aspects.enUS.json",
                "D4Companion/Data/Aspects.enUS.json",
            ),
            (
                "assets/original/data/ItemTypes.enUS.json",
                "D4Companion/Data/ItemTypes.enUS.json",
            ),
            (
                "assets/original/data/ParagonBoards.enUS.json",
                "D4Companion/Data/ParagonBoards.enUS.json",
            ),
            (
                "assets/original/data/ParagonGlyphs.enUS.json",
                "D4Companion/Data/ParagonGlyphs.enUS.json",
            ),
            (
                "assets/original/data/Runes.enUS.json",
                "D4Companion/Data/Runes.enUS.json",
            ),
            (
                "assets/original/data/Sigils.enUS.json",
                "D4Companion/Data/Sigils.enUS.json",
            ),
            (
                "assets/original/data/Uniques.enUS.json",
                "D4Companion/Data/Uniques.enUS.json",
            ),
        ];
        let mut manifest_json = String::from("{\"items\":[");
        let mut provenance_json = String::from("{\"copied_files\":[");
        for (index, (destination_path, source_path)) in entries.iter().enumerate() {
            let sha = file_sha(&root.join(destination_path));
            if index > 0 {
                manifest_json.push(',');
                provenance_json.push(',');
            }
            manifest_json.push_str(&format!(
                "{{\"source_path\":\"{}\",\"sha256\":\"{}\"}}",
                source_path, sha
            ));
            provenance_json.push_str(&format!(
                "{{\"destination_path\":\"{}\",\"source_path\":\"{}\",\"sha256\":\"{}\"}}",
                destination_path, source_path, sha
            ));
        }
        manifest_json.push_str("]}\n");
        provenance_json.push_str("]}\n");
        create_file(&root.join(UPSTREAM_MANIFEST_PATH), &manifest_json);
        create_file(&root.join(UPSTREAM_PROVENANCE_PATH), &provenance_json);

        create_file(
            &root.join("assets/original/data/Aspects.enUS.json"),
            "[{\"id\":1}]\n",
        );

        let result = match upstream_asset_parity_command(&root) {
            Ok(output) => panic!("parity should fail, got success output: {output}"),
            Err(output) => output,
        };
        assert!(result.contains("changed_files=1"));
        assert!(result.contains("missing_files=0"));
        assert!(result.contains("extra_files=0"));
        assert!(result.contains("baseline_enus_failures=1"));
        assert!(result.contains("changed assets/original/data/Aspects.enUS.json"));
        remove_dir(&root);
    }

    #[test]
    fn upstream_asset_parity_fails_when_zip_member_source_is_missing() {
        let root =
            create_temp_repo("upstream_asset_parity_fails_when_zip_member_source_is_missing");
        create_file(
            &root.join(UPSTREAM_CONFIG),
            r#"
[upstream]
url = "https://example.invalid/repo.git"
branch = "master"
local_path = "upstream/original"
[critical_files]
preset_globs = ["downloads/systempresets/*.zip"]
"#,
        );
        create_file(
            &root.join(UPSTREAM_LOCK),
            "last_reviewed_commit = \"abc123\"\nlast_reviewed_release = \"UNKNOWN\"\n",
        );
        create_file(
            &root.join("assets/original/systempresets/2160p_SSF/dot-splitter_2.png"),
            "png\n",
        );
        create_zip_file(
            &root.join("upstream/original/downloads/systempresets/2160p_SSF.zip"),
            &[("2160p_SSF/dot-splitter_1.png", b"png\n")],
        );
        create_file(
            &root.join(UPSTREAM_MANIFEST_PATH),
            "{\"items\":[{\"source_path\":\"downloads/systempresets/2160p_SSF.zip\",\"sha256\":\"archive\"}]}\n",
        );
        create_file(
            &root.join(UPSTREAM_PROVENANCE_PATH),
            "{\"copied_files\":[{\"destination_path\":\"assets/original/systempresets/2160p_SSF/dot-splitter_2.png\",\"source_path\":\"downloads/systempresets/2160p_SSF.zip::2160p_SSF/dot-splitter_2.png\",\"sha256\":\"2d4566582844690f8634a8b2534ea5221560038c6c0650c99140759bad603ae2\"}]}\n",
        );

        let result = match upstream_asset_parity_command(&root) {
            Ok(output) => panic!("parity should fail, got success output: {output}"),
            Err(output) => output,
        };

        assert!(result.contains("manifest_mismatches=1"));
        assert!(result.contains("archive_member=2160p_SSF/dot-splitter_2.png status=MISSING"));
        remove_dir(&root);
    }

    fn create_temp_repo(test_name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "time_error".to_string());
        let root = env::temp_dir().join(format!("d4lc-upstream-{test_name}-{suffix}"));
        create_dir(&root);
        root
    }

    fn create_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            create_dir(parent);
        }
        fs::write(path, contents).unwrap_or_else(|error| {
            panic!("failed to write {}: {error}", path.display());
        });
    }

    fn create_dir(path: &Path) {
        fs::create_dir_all(path).unwrap_or_else(|error| {
            panic!("failed to create {}: {error}", path.display());
        });
    }

    fn write_array_file(root: &Path, relative_path: &str, count: usize) {
        let mut output = String::from("[");
        for index in 0..count {
            if index > 0 {
                output.push(',');
            }
            output.push_str(&index.to_string());
        }
        output.push_str("]\n");
        create_file(&root.join(relative_path), &output);
    }

    fn create_zip_file(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        if let Some(parent) = path.parent() {
            create_dir(parent);
        }
        let file = fs::File::create(path).unwrap_or_else(|error| {
            panic!("failed to create {}: {error}", path.display());
        });
        let mut writer = zip::ZipWriter::new(file);
        for (name, contents) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap_or_else(|error| panic!("failed to start zip member {name}: {error}"));
            writer
                .write_all(contents)
                .unwrap_or_else(|error| panic!("failed to write zip member {name}: {error}"));
        }
        writer
            .finish()
            .unwrap_or_else(|error| panic!("failed to finish zip {}: {error}", path.display()));
    }

    fn file_sha(path: &Path) -> String {
        let bytes = fs::read(path).unwrap_or_else(|error| {
            panic!("failed to read {}: {error}", path.display());
        });
        sha256_hex(&bytes)
    }

    fn remove_dir(path: &Path) {
        fs::remove_dir_all(path).unwrap_or_else(|error| {
            panic!("failed to remove {}: {error}", path.display());
        });
    }
}
