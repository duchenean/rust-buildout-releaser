use crate::error::{ReleaserError, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BuildoutVersions {
    /// Raw content of the file
    content: String,
    /// Parsed versions: package_name -> (version, line_number)
    versions: HashMap<String, (String, usize)>,
    /// File path
    path: String,
}

#[derive(Debug, Clone)]
pub struct VersionUpdate {
    pub package_name: String,
    pub old_version: String,
    pub new_version: String,
}

impl BuildoutVersions {
    /// Load and parse a buildout versions file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let content = std::fs::read_to_string(path.as_ref())?;

        let versions = Self::parse_versions(&content)?;

        Ok(Self {
            content,
            versions,
            path: path_str,
        })
    }

    /// Build a versions snapshot from raw content
    pub fn from_content<S: Into<String>>(content: String, path: S) -> Result<Self> {
        let versions = Self::parse_versions(&content)?;

        Ok(Self {
            content,
            versions,
            path: path.into(),
        })
    }

    /// Parse version pins from buildout cfg content
    fn parse_versions(content: &str) -> Result<HashMap<String, (String, usize)>> {
        let mut versions = HashMap::new();
        let mut in_versions_section = false;

        // Match section headers like [versions] or [versions:python3]
        let section_re = Regex::new(r"^\s*\[([^\]]+)\]\s*$").unwrap();

        // Match version pins like: package.name = 1.2.3
        // Handles various formats: spaces, tabs, comments
        let version_re = Regex::new(r"^\s*([a-zA-Z0-9._-]+)\s*=\s*([^\s#]+)").unwrap();

        for (line_num, line) in content.lines().enumerate() {
            // Skip comments and empty lines
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }

            // Check for section headers
            if let Some(caps) = section_re.captures(line) {
                let section = caps.get(1).unwrap().as_str();
                in_versions_section = section.starts_with("versions");
                continue;
            }

            // Parse version pins in versions section
            if in_versions_section {
                if let Some(caps) = version_re.captures(line) {
                    let package = caps.get(1).unwrap().as_str().to_string();
                    let version = caps.get(2).unwrap().as_str().to_string();
                    versions.insert(package, (version, line_num));
                }
            }
        }

        Ok(versions)
    }

    /// Get the current version of a package
    pub fn get_version(&self, package_name: &str) -> Option<&str> {
        self.versions.get(package_name).map(|(v, _)| v.as_str())
    }

    /// Get all tracked packages and their versions
    pub fn get_all_versions(&self) -> impl Iterator<Item = (&str, &str)> {
        self.versions
            .iter()
            .map(|(k, (v, _))| (k.as_str(), v.as_str()))
    }

    /// Update a package version and return the update info
    pub fn update_version(
        &mut self,
        package_name: &str,
        new_version: &str,
    ) -> Result<Option<VersionUpdate>> {
        let old_version = match self.versions.get(package_name) {
            Some((v, _)) => v.clone(),
            None => return Ok(None), // Package not in file
        };

        if old_version == new_version {
            return Ok(None); // No change needed
        }

        // Create regex to find and replace the version line
        let pattern = format!(
            r"(?m)^(\s*{}\s*=\s*){}(\s*(?:#.*)?)$",
            regex::escape(package_name),
            regex::escape(&old_version)
        );
        let re =
            Regex::new(&pattern).map_err(|e| ReleaserError::BuildoutParseError(e.to_string()))?;

        self.content = re
            .replace(&self.content, format!("${{1}}{}${{2}}", new_version))
            .to_string();

        // Update internal tracking
        if let Some((v, line)) = self.versions.get_mut(package_name) {
            *v = new_version.to_string();
            let _ = line; // Keep line number (it doesn't change)
        }

        Ok(Some(VersionUpdate {
            package_name: package_name.to_string(),
            old_version,
            new_version: new_version.to_string(),
        }))
    }

    /// Add a new package version (if not exists)
    pub fn add_version(&mut self, package_name: &str, version: &str) -> Result<bool> {
        if self.versions.contains_key(package_name) {
            return Ok(false);
        }

        // Find the [versions] section and add at the end of it
        let section_re = Regex::new(r"(?m)^\s*\[versions[^\]]*\]\s*$").unwrap();

        if let Some(mat) = section_re.find(&self.content) {
            // Find the next section or end of file
            let after_section = &self.content[mat.end()..];
            let next_section_re = Regex::new(r"(?m)^\s*\[[^\]]+\]\s*$").unwrap();

            let insert_pos = if let Some(next_mat) = next_section_re.find(after_section) {
                mat.end() + next_mat.start()
            } else {
                self.content.len()
            };

            // Insert the new version line
            let new_line = format!("{} = {}\n", package_name, version);
            self.content.insert_str(insert_pos, &new_line);

            self.versions
                .insert(package_name.to_string(), (version.to_string(), 0));

            Ok(true)
        } else {
            Err(ReleaserError::BuildoutParseError(
                "Could not find [versions] section".to_string(),
            ))
        }
    }

    /// Save the modified content back to the file
    pub fn save(&self) -> Result<()> {
        std::fs::write(&self.path, &self.content)?;
        Ok(())
    }

    /// Save to a different path
    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        std::fs::write(path.as_ref(), &self.content)?;
        Ok(())
    }

    /// Get the raw content
    pub fn content(&self) -> &str {
        &self.content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_versions() {
        let content = r#"
[buildout]
parts =
    app

[versions]
# Some comment
zope.interface = 5.4.0
plone.api = 2.0.0

[versions:python3]
six = 1.16.0
"#;

        let versions = BuildoutVersions::parse_versions(content).unwrap();

        assert_eq!(
            versions.get("zope.interface").map(|(v, _)| v.as_str()),
            Some("5.4.0")
        );
        assert_eq!(
            versions.get("plone.api").map(|(v, _)| v.as_str()),
            Some("2.0.0")
        );
        assert_eq!(versions.get("six").map(|(v, _)| v.as_str()), Some("1.16.0"));
    }
}
