use crate::config::{MetadataFileConfig, VersionBumpType, VersionConfig};
use crate::error::{ReleaserError, Result};
use regex::Regex;
use std::cmp::Ordering;
use std::path::Path;

pub mod python {
    use crate::error::{ReleaserError, Result};
    use regex::Regex;
    use semver::{BuildMetadata, Prerelease};

    /// Parse a Python version string into semver
    pub fn parse_python_version(version: &str) -> Option<semver::Version> {
        // Handle common Python version formats
        // PEP 440: X.Y.Z, X.Y.ZaN, X.Y.ZbN, X.Y.ZrcN, X.Y.Z.postN, X.Y.Z.devN

        let version = version.trim().trim_start_matches('v').replace('_', ".");

        // Try direct semver parse first
        if let Ok(v) = semver::Version::parse(&version) {
            return Some(v);
        }

        let (core, local_suffix) = match version.split_once('+') {
            Some((core, local)) => (core, Some(local)),
            None => (version.as_str(), None),
        };

        // Convert Python-style pre-releases to semver
        let re = Regex::new(concat!(
            r"^(?P<major>\d+)(?:\.(?P<minor>\d+))?(?:\.(?P<patch>\d+))?",
            r"(?P<rest>(?:\.\d+)*)",
            // Pre-release
            r"(?:(?P<pre_sep>[-_.]?)(?P<pre_label>a|b|rc|alpha|beta|c|pre|preview)(?P<pre_num>\d+)?)?",
            // Post release
            r"(?:(?P<post_sep>[-_.]?)(?P<post_label>post|rev|r)(?P<post_num>\d+)?)?",
            // Dev release
            r"(?:(?P<dev_sep>[-_.]?)(?P<dev_label>dev)(?P<dev_num>\d+)?)?$"
        )).ok()?;

        if let Some(caps) = re.captures(core) {
            let major: u64 = caps.name("major")?.as_str().parse().ok()?;
            let minor: u64 = caps
                .name("minor")
                .map(|m| m.as_str().parse().ok())
                .flatten()
                .unwrap_or(0);
            let patch: u64 = caps
                .name("patch")
                .map(|m| m.as_str().parse().ok())
                .flatten()
                .unwrap_or(0);

            let mut pre_parts: Vec<String> = Vec::new();
            if let Some(pre_label) = caps.name("pre_label") {
                let label = match pre_label.as_str() {
                    "a" | "alpha" => "alpha",
                    "b" | "beta" => "beta",
                    "rc" | "c" | "pre" | "preview" => "rc",
                    _ => return None,
                };
                pre_parts.push(label.to_string());

                if let Some(pre_num) = caps.name("pre_num") {
                    pre_parts.push(pre_num.as_str().to_string());
                }
            }

            if let Some(dev_label) = caps.name("dev_label") {
                pre_parts.push(dev_label.as_str().to_string());
                if let Some(dev_num) = caps.name("dev_num") {
                    pre_parts.push(dev_num.as_str().to_string());
                }
            }

            let pre = if pre_parts.is_empty() {
                Prerelease::EMPTY
            } else {
                Prerelease::new(&pre_parts.join(".")).ok()?
            };

            let mut build_parts: Vec<String> = caps
                .name("rest")
                .map(|m| {
                    m.as_str()
                        .trim_start_matches('.')
                        .split('.')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.replace('_', "-"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if caps.name("post_label").is_some() {
                let post_num = caps.name("post_num").map(|m| m.as_str()).unwrap_or("0");
                build_parts.push(format!("post{}", post_num));
            }

            if let Some(local) = local_suffix {
                if !local.is_empty() {
                    build_parts.push(local.replace('_', "-"));
                }
            }

            let build = if build_parts.is_empty() {
                BuildMetadata::EMPTY
            } else {
                BuildMetadata::new(&build_parts.join(".")).ok()?
            };

            return Some(semver::Version {
                major,
                minor,
                patch,
                pre,
                build,
            });
        }

        None
    }

    /// Parse a Python version constraint to semver requirement
    pub fn parse_version_constraint(
        constraint: &str,
    ) -> Result<(semver::VersionReq, Vec<(semver::Version, semver::Version)>)> {
        // Convert Python-style constraints to semver
        // ~=X.Y -> >=X.Y.0, <X+1.0.0 (approximately)
        // ==X.Y.Z -> =X.Y.Z
        // >=X.Y.Z -> >=X.Y.Z
        // etc.

        let constraint = constraint.trim();

        if constraint.is_empty() {
            return Err(ReleaserError::VersionError(
                "Empty version constraint".to_string(),
            ));
        }

        // Handle ~= (compatible release)
        if constraint.contains("||") {
            return Err(ReleaserError::VersionError(
                "OR (||) constraints are not supported".to_string(),
            ));
        }

        if constraint.starts_with("~=") {
            let version = constraint[2..].trim();
            let parsed = parse_python_version(version)
                .ok_or_else(|| ReleaserError::VersionError(version.to_string()))?;

            let release_len = Regex::new(r"^(\d+(?:\.\d+)*)")
                .ok()
                .and_then(|re| re.captures(version))
                .map(|caps| caps[1].split('.').count())
                .unwrap_or(0);

            let upper_bound = match release_len {
                0 | 1 | 2 => format!("{}.0.0", parsed.major + 1),
                _ => format!("{}.{}.0", parsed.major, parsed.minor + 1),
            };

            let req = semver::VersionReq::parse(&format!(">={}, <{}", parsed, upper_bound))
                .map_err(|e| ReleaserError::VersionError(e.to_string()))?;

            return Ok((req, Vec::new()));
        }

        let mut exclusions = Vec::new();

        let parts: Result<Vec<String>> = constraint
            .split(',')
            .map(|raw| {
                let (expr, mut excluded) = normalize_constraint_part(raw)?;
                exclusions.append(&mut excluded);
                Ok(expr)
            })
            .collect();

        let normalized = parts?.join(", ");

        let req = semver::VersionReq::parse(&normalized)
            .map_err(|e| ReleaserError::VersionError(format!("{}: {}", normalized, e)))?;

        Ok((req, exclusions))
    }

    pub fn normalize_constraint_part(
        part: &str,
    ) -> Result<(String, Vec<(semver::Version, semver::Version)>)> {
        let part = part.trim();

        if part.is_empty() {
            return Err(ReleaserError::VersionError(
                "Empty constraint segment".to_string(),
            ));
        }

        // Wildcard equality (==1.2.*)
        let wildcard_re = Regex::new(r"^(==|!=)\s*(\d+)(?:\.(\d+))?\.\*$")
            .map_err(|e| ReleaserError::VersionError(e.to_string()))?;

        if let Some(caps) = wildcard_re.captures(part) {
            let op = caps.get(1).map(|m| m.as_str()).unwrap_or("==");
            let major: u64 = caps[2]
                .parse()
                .map_err(|_| ReleaserError::VersionError(part.to_string()))?;
            let minor: Option<u64> = caps.get(3).map(|m| m.as_str().parse().ok()).flatten();

            let normalized_op = if op == "==" { "=" } else { op };

            let (lower, upper) = if let Some(minor) = minor {
                (
                    semver::Version::new(major, minor, 0),
                    semver::Version::new(major, minor + 1, 0),
                )
            } else {
                (
                    semver::Version::new(major, 0, 0),
                    semver::Version::new(major + 1, 0, 0),
                )
            };

            let expr = match normalized_op {
                "=" => format!(">={}, <{}", lower, upper),
                "!=" => "*".to_string(),
                _ => part.to_string(),
            };

            let mut exclusions = Vec::new();
            if normalized_op == "!=" {
                exclusions.push((lower, upper));
            }

            return Ok((expr, exclusions));
        }

        let re = Regex::new(r"^(<=|>=|==|===|!=|<|>|=)?\s*(.+)$")
            .map_err(|e| ReleaserError::VersionError(e.to_string()))?;

        if let Some(caps) = re.captures(part) {
            let op = caps.get(1).map(|m| m.as_str()).unwrap_or("=");
            let version_str = caps.get(2).map(|m| m.as_str()).unwrap_or("");

            let parsed = parse_python_version(version_str)
                .ok_or_else(|| ReleaserError::VersionError(part.to_string()))?;

            let normalized_op = match op {
                "==" | "=" | "===" => "=",
                other => other,
            };

            return Ok((format!("{}{}", normalized_op, parsed), Vec::new()));
        }

        Err(ReleaserError::VersionError(part.to_string()))
    }
}

/// Semantic version representation backed by the semver crate
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    inner: semver::Version,
}

impl Version {
    /// Parse a version string
    pub fn parse(s: &str) -> Result<Self> {
        let normalized = s.trim().trim_start_matches('v');

        let parsed = semver::Version::parse(normalized)
            .or_else(|_| {
                python::parse_python_version(normalized)
                    .ok_or_else(|| ReleaserError::VersionError(normalized.to_string()))
            })?;

        Ok(Self { inner: parsed })
    }

    /// Create a new version
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            inner: semver::Version::new(major as u64, minor as u64, patch as u64),
        }
    }

    /// Bump the version according to the bump type
    pub fn bump(&self, bump_type: VersionBumpType) -> Self {
        let mut bumped = self.inner.clone();

        match bump_type {
            VersionBumpType::Major => {
                bumped.major += 1;
                bumped.minor = 0;
                bumped.patch = 0;
            }
            VersionBumpType::Minor => {
                bumped.minor += 1;
                bumped.patch = 0;
            }
            VersionBumpType::Patch => {
                bumped.patch += 1;
            }
        }

        bumped.pre = semver::Prerelease::EMPTY;
        bumped.build = semver::BuildMetadata::EMPTY;

        Self { inner: bumped }
    }

    /// Get the major component
    pub fn major(&self) -> u64 {
        self.inner.major
    }

    /// Get the minor component
    pub fn minor(&self) -> u64 {
        self.inner.minor
    }

    /// Get the patch component
    pub fn patch(&self) -> u64 {
        self.inner.patch
    }

    /// Get prerelease identifier if present
    pub fn prerelease(&self) -> Option<&str> {
        if self.inner.pre.is_empty() {
            None
        } else {
            Some(self.inner.pre.as_str())
        }
    }

    /// Get build metadata if present
    pub fn build_metadata(&self) -> Option<&str> {
        if self.inner.build.is_empty() {
            None
        } else {
            Some(self.inner.build.as_str())
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.inner.cmp(&other.inner)
    }
}

#[cfg(test)]
mod python_tests {
    use super::python::{
        normalize_constraint_part, parse_python_version, parse_version_constraint,
    };

    #[test]
    fn parses_additional_python_versions() {
        let v = parse_python_version("1.2").expect("should parse minor-only");
        assert_eq!(v.to_string(), "1.2.0");

        let v = parse_python_version("2.0rc1").expect("should parse rc prerelease");
        assert_eq!(v.to_string(), "2.0.0-rc.1");

        let v = parse_python_version("3.4.post2").expect("should parse post release");
        assert_eq!(v.to_string(), "3.4.0+post2");

        let v = parse_python_version("4.5.dev7").expect("should parse dev prerelease");
        assert_eq!(v.to_string(), "4.5.0-dev.7");

        let v = parse_python_version("1.0+local.tag").expect("should parse local metadata");
        assert_eq!(v.to_string(), "1.0.0+local.tag");

        let v = parse_python_version("4.2.3.1").expect("should parse four-segment release");
        assert_eq!(v.to_string(), "4.2.3+1");

        let v = parse_python_version("4.2.3.1rc1").expect("should parse four-segment rc prerelease");
        assert_eq!(v.to_string(), "4.2.3-rc.1+1");

        let v = parse_python_version("4.2.3.28b3").expect("should parse four-segment beta prerelease");
        assert_eq!(v.to_string(), "4.2.3-beta.3+28");

        let v = parse_python_version("4.2.4.8a2").expect("should parse four-segment alpha prerelease");
        assert_eq!(v.to_string(), "4.2.4-alpha.2+8");

        let v = parse_python_version("2.5").expect("should parse two-segment release");
        assert_eq!(v.to_string(), "2.5.0");

        let v = parse_python_version("7").expect("should parse single-segment release");
        assert_eq!(v.to_string(), "7.0.0");
    }

    #[test]
    fn parses_wildcard_constraints() {
        let (req, exclusions) =
            parse_version_constraint("==3.8.*").expect("should parse wildcard equality");
        let matches = req.matches(&semver::Version::parse("3.8.5").unwrap());
        assert!(matches, "should accept version within wildcard range");
        assert!(exclusions.is_empty());

        let (req, exclusions) =
            parse_version_constraint("!=2.*").expect("should parse wildcard inequality");
        assert!(req.matches(&semver::Version::parse("1.9.9").unwrap()));
        assert_eq!(exclusions.len(), 1);
        let (lower, upper) = &exclusions[0];
        assert_eq!(lower, &semver::Version::new(2, 0, 0));
        assert_eq!(upper, &semver::Version::new(3, 0, 0));
    }

    #[test]
    fn parses_partial_comparators() {
        let (req, exclusions) =
            parse_version_constraint(">=3.8").expect("should parse partial comparator");
        assert!(req.matches(&semver::Version::parse("3.8.1").unwrap()));
        assert!(exclusions.is_empty());

        let (req, exclusions) =
            parse_version_constraint("~=3.8").expect("should parse compatible release");
        assert!(req.matches(&semver::Version::parse("3.8.9").unwrap()));
        assert!(!req.matches(&semver::Version::parse("4.0.0").unwrap()));
        assert!(exclusions.is_empty());
    }

    #[test]
    fn normalizes_constraint_parts() {
        let (normalized, exclusions) = normalize_constraint_part("==1.2").unwrap();
        assert_eq!(normalized, "=1.2.0");
        assert!(exclusions.is_empty());

        let (normalized, exclusions) = normalize_constraint_part("< 1").unwrap();
        assert_eq!(normalized, "<1.0.0");
        assert!(exclusions.is_empty());
    }
}

/// Version manager for reading/writing/bumping versions
pub struct VersionManager<'a> {
    config: &'a VersionConfig,
}

impl<'a> VersionManager<'a> {
    pub fn new(config: &'a VersionConfig) -> Self {
        Self { config }
    }

    /// Get bump type from level name
    pub fn get_bump_type(&self, level: &str) -> Result<VersionBumpType> {
        self.config.levels.get(level).copied().ok_or_else(|| {
            let available: Vec<_> = self.config.levels.keys().collect();
            ReleaserError::VersionError(format!(
                "Unknown version level '{}'. Available: {:?}",
                level, available
            ))
        })
    }

    /// List available version levels
    pub fn available_levels(&self) -> Vec<(&str, VersionBumpType)> {
        self.config
            .levels
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect()
    }
}

/// Metadata file updater
pub struct MetadataUpdater;

impl MetadataUpdater {
    /// Update a metadata file with new version and date
    pub fn update_file(config: &MetadataFileConfig, version: &str, date: &str) -> Result<()> {
        let path = Path::new(&config.path);

        if !path.exists() {
            return Err(ReleaserError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Metadata file not found: {}", config.path),
            )));
        }

        match config.format.to_lowercase().as_str() {
            "yaml" | "yml" => Self::update_yaml(config, version, date),
            "json" => Self::update_json(config, version, date),
            "toml" => Self::update_toml(config, version, date),
            _ => Err(ReleaserError::ConfigError(format!(
                "Unsupported metadata format: {}",
                config.format
            ))),
        }
    }

    /// Update YAML file
    fn update_yaml(config: &MetadataFileConfig, version: &str, date: &str) -> Result<()> {
        let content = std::fs::read_to_string(&config.path)?;
        let mut new_content = content.clone();

        // Update version fields
        for field in &config.version_fields {
            new_content = Self::update_yaml_field(&new_content, field, version);
        }

        // Update date fields
        for field in &config.date_fields {
            new_content = Self::update_yaml_field(&new_content, field, date);
        }

        std::fs::write(&config.path, new_content)?;
        Ok(())
    }

    /// Update a single YAML field
    fn update_yaml_field(content: &str, field: &str, value: &str) -> String {
        let escaped_field = regex::escape(field);

        // Handle double-quoted values: field: "value"
        let pattern_double_quote = format!(r#"(?m)^(\s*{}:\s*)"([^"]*)""#, escaped_field);
        if let Ok(re) = Regex::new(&pattern_double_quote) {
            if re.is_match(content) {
                let replacement = format!(r#"${{1}}"{}""#, value);
                return re.replace(content, replacement.as_str()).to_string();
            }
        }

        // Handle single-quoted values: field: 'value'
        let pattern_single_quote = format!(r"(?m)^(\s*{}:\s*)'([^']*)'", escaped_field);
        if let Ok(re) = Regex::new(&pattern_single_quote) {
            if re.is_match(content) {
                let replacement = format!("${{1}}'{}'", value);
                return re.replace(content, replacement.as_str()).to_string();
            }
        }

        // Handle unquoted values: field: value
        let pattern_unquoted = format!(r"(?m)^(\s*{}:\s*)([^\s#\n][^\n]*)", escaped_field);
        if let Ok(re) = Regex::new(&pattern_unquoted) {
            if re.is_match(content) {
                let replacement = format!("${{1}}{}", value);
                return re.replace(content, replacement.as_str()).to_string();
            }
        }

        content.to_string()
    }

    /// Update JSON file
    fn update_json(config: &MetadataFileConfig, version: &str, date: &str) -> Result<()> {
        let content = std::fs::read_to_string(&config.path)?;
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ReleaserError::ConfigError(format!("Invalid JSON: {}", e)))?;

        // Update version fields
        for field in &config.version_fields {
            Self::set_json_field(&mut json, field, version);
        }

        // Update date fields
        for field in &config.date_fields {
            Self::set_json_field(&mut json, field, date);
        }

        let new_content = serde_json::to_string_pretty(&json)
            .map_err(|e| ReleaserError::ConfigError(format!("Failed to serialize JSON: {}", e)))?;

        std::fs::write(&config.path, new_content)?;
        Ok(())
    }

    /// Set a field in JSON (supports nested paths like "info.version")
    fn set_json_field(json: &mut serde_json::Value, field: &str, value: &str) {
        let parts: Vec<&str> = field.split('.').collect();

        // Navigate to the parent and set the final key
        let mut current = json;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last part - set the value
                if let serde_json::Value::Object(obj) = current {
                    obj.insert(
                        part.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
            } else {
                // Navigate deeper, creating objects as needed
                if current.get(*part).is_none() {
                    if let serde_json::Value::Object(obj) = current {
                        obj.insert(part.to_string(), serde_json::json!({}));
                    }
                }
                current = current.get_mut(*part).unwrap();
            }
        }
    }

    /// Update TOML file
    fn update_toml(config: &MetadataFileConfig, version: &str, date: &str) -> Result<()> {
        let content = std::fs::read_to_string(&config.path)?;
        let mut toml_value: toml::Value = content
            .parse()
            .map_err(|e| ReleaserError::ConfigError(format!("Invalid TOML: {}", e)))?;

        // Update version fields
        for field in &config.version_fields {
            Self::set_toml_field(&mut toml_value, field, version);
        }

        // Update date fields
        for field in &config.date_fields {
            Self::set_toml_field(&mut toml_value, field, date);
        }

        let new_content = toml::to_string_pretty(&toml_value)
            .map_err(|e| ReleaserError::ConfigError(format!("Failed to serialize TOML: {}", e)))?;

        std::fs::write(&config.path, new_content)?;
        Ok(())
    }

    /// Set a field in TOML (supports nested paths)
    fn set_toml_field(toml_value: &mut toml::Value, field: &str, value: &str) {
        let parts: Vec<&str> = field.split('.').collect();

        let mut current = toml_value;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last part - set the value
                if let toml::Value::Table(table) = current {
                    table.insert(part.to_string(), toml::Value::String(value.to_string()));
                }
            } else {
                // Navigate deeper, creating tables as needed
                if current.get(*part).is_none() {
                    if let toml::Value::Table(table) = current {
                        table.insert(part.to_string(), toml::Value::Table(toml::map::Map::new()));
                    }
                }
                current = current.get_mut(*part).unwrap();
            }
        }
    }

    /// Update all configured metadata files
    pub fn update_all(
        configs: &[MetadataFileConfig],
        version: &str,
        date: &str,
    ) -> Result<Vec<String>> {
        let mut updated_files = Vec::new();

        for config in configs {
            match Self::update_file(config, version, date) {
                Ok(()) => {
                    updated_files.push(config.path.clone());
                }
                Err(e) => {
                    eprintln!("Warning: Failed to update {}: {}", config.path, e);
                }
            }
        }

        Ok(updated_files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parse() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.major(), 1);
        assert_eq!(v.minor(), 2);
        assert_eq!(v.patch(), 3);
        assert_eq!(v.prerelease(), None);

        let v = Version::parse("v2.0.0-beta.1").unwrap();
        assert_eq!(v.major(), 2);
        assert_eq!(v.minor(), 0);
        assert_eq!(v.patch(), 0);
        assert_eq!(v.prerelease(), Some("beta.1"));

        let v = Version::parse("1.2.3+build.5").unwrap();
        assert_eq!(v.build_metadata(), Some("build.5"));
        assert_eq!(v.to_string(), "1.2.3+build.5");

        let v = Version::parse("4.2.3.1").unwrap();
        assert_eq!(v.to_string(), "4.2.3+1");

        // Also support X.Y format
        let v = Version::parse("1.2").unwrap();
        assert_eq!(v.major(), 1);
        assert_eq!(v.minor(), 2);
        assert_eq!(v.patch(), 0);
    }

    #[test]
    fn test_version_bump() {
        let v = Version::parse("1.2.3").unwrap();

        let major = v.bump(VersionBumpType::Major);
        assert_eq!(major.to_string(), "2.0.0");

        let minor = v.bump(VersionBumpType::Minor);
        assert_eq!(minor.to_string(), "1.3.0");

        let patch = v.bump(VersionBumpType::Patch);
        assert_eq!(patch.to_string(), "1.2.4");
    }

    #[test]
    fn test_version_ordering() {
        let v1 = Version::parse("1.0.0").unwrap();
        let v2 = Version::parse("1.0.1").unwrap();
        let v3 = Version::parse("1.1.0").unwrap();
        let v4 = Version::parse("2.0.0").unwrap();
        let v5 = Version::parse("1.0.0-alpha").unwrap();

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert!(v5 < v1); // Pre-release is less than release
    }
}
