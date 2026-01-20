use crate::error::{ReleaserError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    /// Path to the buildout versions file (e.g., versions.cfg)
    pub versions_file: String,

    /// List of packages to track and update
    pub packages: Vec<PackageConfig>,

    /// Git configuration
    #[serde(default)]
    pub git: GitConfig,

    /// GitHub configuration
    #[serde(default)]
    pub github: GitHubConfig,

    /// Changelog configuration
    #[serde(default)]
    pub changelog: ChangelogConfig,

    /// Version configuration
    #[serde(default)]
    pub version: VersionConfig,

    /// Metadata files to update (like publiccode.yml)
    #[serde(default)]
    pub metadata_files: Vec<MetadataFileConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PackageConfig {
    /// Package name on PyPI
    pub name: String,

    /// Optional: pin to a specific version constraint
    #[serde(default)]
    pub version_constraint: Option<String>,

    /// Optional: custom name in buildout if different from PyPI name
    #[serde(default)]
    pub buildout_name: Option<String>,

    /// Whether to include pre-releases
    #[serde(default)]
    pub allow_prerelease: bool,

    /// Optional: custom changelog URL for this package
    #[serde(default)]
    pub changelog_url: Option<String>,

    /// Whether to include this package in consolidated changelog output
    #[serde(default = "default_true")]
    pub include_in_changelog: bool,
}

impl PackageConfig {
    pub fn buildout_name(&self) -> &str {
        self.buildout_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitConfig {
    /// Branch to commit to (default: current branch)
    #[serde(default)]
    pub branch: Option<String>,

    /// Whether to automatically push after commit
    #[serde(default)]
    pub auto_push: bool,

    /// Commit message template
    #[serde(default = "default_commit_template")]
    pub commit_template: String,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            branch: None,
            auto_push: false,
            commit_template: default_commit_template(),
        }
    }
}

impl GitConfig {
    pub fn effective_commit_template(&self) -> &str {
        if self.commit_template.trim().is_empty() {
            "Use {packages}"
        } else {
            &self.commit_template
        }
    }
}

fn default_commit_template() -> String {
    "Use {packages}".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GitHubConfig {
    /// Repository in format "owner/repo"
    #[serde(default)]
    pub repository: Option<String>,

    /// Whether to create a GitHub release after tagging
    #[serde(default)]
    pub create_release: bool,

    /// Tag prefix (e.g., "v" for v1.0.0)
    #[serde(default)]
    pub tag_prefix: String,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            repository: None,
            create_release: true,
            tag_prefix: String::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChangelogConfig {
    /// Whether to collect changelogs by default
    #[serde(default)]
    pub enabled: bool,

    /// Output format: "markdown", "rst", or "text"
    #[serde(default = "default_changelog_format")]
    pub format: String,

    /// Output file path
    #[serde(default)]
    pub output_file: Option<String>,

    /// Whether to include the changelog file in the commit
    #[serde(default = "default_true")]
    pub include_in_commit: bool,

    /// Whether to use changelog as GitHub release notes
    #[serde(default = "default_true")]
    pub use_as_release_notes: bool,

    /// Custom header template
    #[serde(default = "default_changelog_header")]
    pub header_template: String,

    /// Custom section template for each package
    #[serde(default = "default_package_template")]
    pub package_template: String,

    /// Files to look for when fetching changelogs
    #[serde(default = "default_changelog_files")]
    pub changelog_files: Vec<String>,

    /// Additional GitHub branches to try
    #[serde(default)]
    pub github_branches: Vec<String>,
}

fn default_changelog_format() -> String {
    "markdown".to_string()
}

fn default_true() -> bool {
    true
}

fn default_changelog_header() -> String {
    "# Release {version}\n\n**Date:** {date}\n\n## Package Updates".to_string()
}

fn default_package_template() -> String {
    "### {package} ({old_version} → {new_version})".to_string()
}

fn default_changelog_files() -> Vec<String> {
    vec![
        "CHANGELOG.md".to_string(),
        "CHANGES.md".to_string(),
        "HISTORY.md".to_string(),
        "CHANGELOG.rst".to_string(),
        "CHANGES.rst".to_string(),
        "HISTORY.rst".to_string(),
        "CHANGELOG.txt".to_string(),
        "CHANGES.txt".to_string(),
        "HISTORY.txt".to_string(),
        "docs/CHANGELOG.md".to_string(),
        "docs/CHANGES.md".to_string(),
        "docs/changelog.md".to_string(),
        "docs/changes.md".to_string(),
        "docs/history.md".to_string(),
    ]
}

impl Default for ChangelogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            format: default_changelog_format(),
            output_file: Some("CHANGELOG.md".to_string()), // Now has a default
            include_in_commit: true,
            use_as_release_notes: true,
            header_template: default_changelog_header(),
            package_template: default_package_template(),
            changelog_files: default_changelog_files(),
            github_branches: Vec::new(),
        }
    }
}

impl ChangelogConfig {
    pub fn format_enum(&self) -> ChangelogFormat {
        match self.format.to_lowercase().as_str() {
            "rst" | "restructuredtext" => ChangelogFormat::Rst,
            "text" | "txt" | "plain" => ChangelogFormat::Text,
            _ => ChangelogFormat::Markdown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangelogFormat {
    Markdown,
    Rst,
    Text,
}

// ============================================================================
// Version Configuration
// ============================================================================

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VersionConfig {
    /// Version bump levels (customizable names)
    #[serde(default = "default_version_levels")]
    pub levels: HashMap<String, VersionBumpType>,
}

fn default_version_pattern() -> String {
    r#"(?m)^version\s*=\s*["']?(\d+\.\d+\.\d+)["']?"#.to_string()
}

fn default_version_levels() -> HashMap<String, VersionBumpType> {
    let mut levels = HashMap::new();
    levels.insert("major".to_string(), VersionBumpType::Major);
    levels.insert("minor".to_string(), VersionBumpType::Minor);
    levels.insert("patch".to_string(), VersionBumpType::Patch);
    levels.insert("fix".to_string(), VersionBumpType::Patch);
    levels.insert("hotfix".to_string(), VersionBumpType::Patch);
    levels.insert("feature".to_string(), VersionBumpType::Minor);
    levels.insert("breaking".to_string(), VersionBumpType::Major);
    levels
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self {
            levels: default_version_levels(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionBumpType {
    Major,
    Minor,
    Patch,
}

// ============================================================================
// Metadata File Configuration
// ============================================================================

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MetadataFileConfig {
    /// Path to the metadata file
    pub path: String,

    /// File format: "yaml", "json", "toml"
    #[serde(default = "default_metadata_format")]
    pub format: String,

    /// Fields to update with version
    #[serde(default = "default_version_fields")]
    pub version_fields: Vec<String>,

    /// Fields to update with release date (YYYY-MM-DD)
    #[serde(default = "default_date_fields")]
    pub date_fields: Vec<String>,

    /// Whether to include this file in the commit
    #[serde(default = "default_true")]
    pub include_in_commit: bool,
}

fn default_metadata_format() -> String {
    "yaml".to_string()
}

fn default_version_fields() -> Vec<String> {
    vec!["softwareVersion".to_string(), "version".to_string()]
}

fn default_date_fields() -> Vec<String> {
    vec!["releaseDate".to_string()]
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| ReleaserError::ConfigError(format!("Failed to read config: {}", e)))?;

        toml::from_str(&content)
            .map_err(|e| ReleaserError::ConfigError(format!("Failed to parse config: {}", e)))
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self).map_err(|e| {
            ReleaserError::ConfigError(format!("Failed to serialize config: {}", e))
        })?;

        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    pub fn create_default<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config = Config {
            versions_file: "versions.cfg".to_string(),
            packages: vec![PackageConfig {
                name: "example-package".to_string(),
                version_constraint: None,
                buildout_name: None,
                allow_prerelease: false,
                changelog_url: None,
                include_in_changelog: true,
            }],
            git: GitConfig::default(),
            github: GitHubConfig::default(),
            changelog: ChangelogConfig::default(),
            version: VersionConfig::default(),
            metadata_files: vec![MetadataFileConfig {
                path: "publiccode.yml".to_string(),
                format: "yaml".to_string(),
                version_fields: vec!["softwareVersion".to_string()],
                date_fields: vec!["releaseDate".to_string()],
                include_in_commit: true,
            }],
        };

        config.save(path)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_load_config_include_in_changelog() {
        let toml_content = r#"
versions_file = "versions.cfg"

[[packages]]
name = "plonemeeting.portal.core"
allow_prerelease = false
include_in_changelog = true

[[packages]]
name = "plonetheme.deliberations"
allow_prerelease = false
include_in_changelog = false

[[packages]]
name = "collective.timestamp"
allow_prerelease = false
"#;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bldr-config-{}.toml", timestamp));

        fs::write(&path, toml_content).expect("write temp config");
        let config = Config::load(&path).expect("load config");
        fs::remove_file(&path).ok();

        assert_eq!(config.packages.len(), 3);
        assert!(config.packages[0].include_in_changelog);
        assert!(!config.packages[1].include_in_changelog);
        assert!(config.packages[2].include_in_changelog);
    }
}
