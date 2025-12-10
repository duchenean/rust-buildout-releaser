use regex::Regex;
use std::cmp::Ordering;
use std::path::Path;
use crate::config::{MetadataFileConfig, VersionBumpType, VersionConfig};
use crate::error::{ReleaserError, Result};

/// Semantic version representation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub prerelease: Option<String>,
}

impl Version {
    /// Parse a version string
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim().trim_start_matches('v');

        // Pattern: major.minor.patch[-prerelease]
        let re = Regex::new(r"^(\d+)\.(\d+)(?:\.(\d+))?(?:-(.+))?$")
            .map_err(|e| ReleaserError::VersionError(e.to_string()))?;

        let caps = re.captures(s).ok_or_else(|| {
            ReleaserError::VersionError(format!("Invalid version format: {}", s))
        })?;

        Ok(Self {
            major: caps[1].parse().unwrap(),
            minor: caps[2].parse().unwrap(),
            patch: caps.get(3).map(|m| m.as_str().parse().unwrap()).unwrap_or(0),
            prerelease: caps.get(4).map(|m| m.as_str().to_string()),
        })
    }

    /// Create a new version
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            prerelease: None,
        }
    }

    /// Bump the version according to the bump type
    pub fn bump(&self, bump_type: VersionBumpType) -> Self {
        match bump_type {
            VersionBumpType::Major => Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
                prerelease: None,
            },
            VersionBumpType::Minor => Self {
                major: self.major,
                minor: self.minor + 1,
                patch: 0,
                prerelease: None,
            },
            VersionBumpType::Patch => Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch + 1,
                prerelease: None,
            },
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.prerelease {
            Some(pre) => write!(f, "{}.{}.{}-{}", self.major, self.minor, self.patch, pre),
            None => write!(f, "{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.major.cmp(&other.major) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.minor.cmp(&other.minor) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.patch.cmp(&other.patch) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // Pre-release versions are less than release versions
        // e.g., 1.0.0-alpha < 1.0.0
        match (&self.prerelease, &other.prerelease) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
        }
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
    pub fn update_file(
        config: &MetadataFileConfig,
        version: &str,
        date: &str,
    ) -> Result<()> {
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
                    obj.insert(part.to_string(), serde_json::Value::String(value.to_string()));
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
        let mut toml_value: toml::Value = content.parse()
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
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.prerelease, None);

        let v = Version::parse("v2.0.0-beta.1").unwrap();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
        assert_eq!(v.prerelease, Some("beta.1".to_string()));

        // Also support X.Y format
        let v = Version::parse("1.2").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 0);
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