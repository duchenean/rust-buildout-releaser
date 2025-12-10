use serde::Deserialize;
use crate::error::{ReleaserError, Result};

#[derive(Debug, Deserialize)]
pub struct PyPiPackageInfo {
    pub info: PackageInfo,
    pub releases: std::collections::HashMap<String, Vec<ReleaseInfo>>,
}

#[derive(Debug, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub summary: Option<String>,
    pub home_page: Option<String>,
    pub project_urls: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseInfo {
    pub filename: String,
    pub url: String,
    pub upload_time: String,
    pub yanked: bool,
}

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub package_name: String,
    pub version: String,
    pub is_prerelease: bool,
}

pub struct PyPiClient {
    client: reqwest::Client,
    base_url: String,
}

impl PyPiClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("buildout-releaser/0.1.0")
                .build()
                .expect("Failed to create HTTP client"),
            base_url: "https://pypi.org/pypi".to_string(),
        }
    }

    /// Fetch package information from PyPI
    pub async fn get_package_info(&self, package_name: &str) -> Result<PyPiPackageInfo> {
        let url = format!("{}/{}/json", self.base_url, package_name);

        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ReleaserError::PackageNotFound(package_name.to_string()));
        }

        if !response.status().is_success() {
            return Err(ReleaserError::PyPiError(format!(
                "HTTP {} for package {}",
                response.status(),
                package_name
            )));
        }

        response.json::<PyPiPackageInfo>().await
            .map_err(|e| ReleaserError::PyPiError(format!("Failed to parse response: {}", e)))
    }

    /// Get the latest version of a package
    pub async fn get_latest_version(
        &self,
        package_name: &str,
        allow_prerelease: bool
    ) -> Result<VersionInfo> {
        let info = self.get_package_info(package_name).await?;

        // Get all non-yanked versions
        let mut versions: Vec<(semver::Version, String)> = info.releases
            .iter()
            .filter(|(_, releases)| !releases.is_empty() && !releases.iter().all(|r| r.yanked))
            .filter_map(|(version_str, _)| {
                // Try to parse as semver, handle non-standard versions
                parse_python_version(version_str).map(|v| (v, version_str.clone()))
            })
            .collect();

        if !allow_prerelease {
            versions.retain(|(v, _)| v.pre.is_empty());
        }

        versions.sort_by(|a, b| b.0.cmp(&a.0));

        let (parsed_version, version_str) = versions.into_iter().next()
            .ok_or_else(|| ReleaserError::PyPiError(
                format!("No valid versions found for {}", package_name)
            ))?;

        Ok(VersionInfo {
            package_name: info.info.name,
            version: version_str,
            is_prerelease: !parsed_version.pre.is_empty(),
        })
    }

    /// Get versions matching a constraint
    pub async fn get_matching_version(
        &self,
        package_name: &str,
        constraint: &str,
        allow_prerelease: bool,
    ) -> Result<VersionInfo> {
        let info = self.get_package_info(package_name).await?;
        let req = parse_version_constraint(constraint)?;

        let mut versions: Vec<(semver::Version, String)> = info.releases
            .iter()
            .filter(|(_, releases)| !releases.is_empty() && !releases.iter().all(|r| r.yanked))
            .filter_map(|(version_str, _)| {
                parse_python_version(version_str).map(|v| (v, version_str.clone()))
            })
            .filter(|(v, _)| req.matches(v))
            .collect();

        if !allow_prerelease {
            versions.retain(|(v, _)| v.pre.is_empty());
        }

        versions.sort_by(|a, b| b.0.cmp(&a.0));

        let (parsed_version, version_str) = versions.into_iter().next()
            .ok_or_else(|| ReleaserError::PyPiError(
                format!("No versions matching '{}' for {}", constraint, package_name)
            ))?;

        Ok(VersionInfo {
            package_name: info.info.name,
            version: version_str,
            is_prerelease: !parsed_version.pre.is_empty(),
        })
    }
}

/// Parse a Python version string into semver
fn parse_python_version(version: &str) -> Option<semver::Version> {
    // Handle common Python version formats
    // PEP 440: X.Y.Z, X.Y.ZaN, X.Y.ZbN, X.Y.ZrcN, X.Y.Z.postN, X.Y.Z.devN

    let version = version.trim();

    // Try direct semver parse first
    if let Ok(v) = semver::Version::parse(version) {
        return Some(v);
    }

    // Convert Python-style pre-releases to semver
    let re = regex::Regex::new(
        r"^(\d+)\.(\d+)(?:\.(\d+))?(?:(a|b|rc|alpha|beta|dev|post)(\d+))?$"
    ).ok()?;

    if let Some(caps) = re.captures(version) {
        let major: u64 = caps.get(1)?.as_str().parse().ok()?;
        let minor: u64 = caps.get(2)?.as_str().parse().ok()?;
        let patch: u64 = caps.get(3).map(|m| m.as_str().parse().ok()).flatten().unwrap_or(0);

        let pre = if let (Some(pre_type), Some(pre_num)) = (caps.get(4), caps.get(5)) {
            let pre_type = match pre_type.as_str() {
                "a" | "alpha" => "alpha",
                "b" | "beta" => "beta",
                "rc" => "rc",
                "dev" => "dev",
                "post" => "post",
                _ => return None,
            };
            semver::Prerelease::new(&format!("{}.{}", pre_type, pre_num.as_str())).ok()?
        } else {
            semver::Prerelease::EMPTY
        };

        return Some(semver::Version {
            major,
            minor,
            patch,
            pre,
            build: semver::BuildMetadata::EMPTY,
        });
    }

    None
}

/// Parse a Python version constraint to semver requirement
fn parse_version_constraint(constraint: &str) -> Result<semver::VersionReq> {
    // Convert Python-style constraints to semver
    // ~=X.Y -> >=X.Y.0, <X+1.0.0 (approximately)
    // ==X.Y.Z -> =X.Y.Z
    // >=X.Y.Z -> >=X.Y.Z
    // etc.

    let constraint = constraint.trim();

    // Handle ~= (compatible release)
    if constraint.starts_with("~=") {
        let version = constraint[2..].trim();
        let parts: Vec<&str> = version.split('.').collect();

        match parts.len() {
            2 => {
                let major: u64 = parts[0].parse()
                    .map_err(|_| ReleaserError::VersionError(constraint.to_string()))?;
                let minor: u64 = parts[1].parse()
                    .map_err(|_| ReleaserError::VersionError(constraint.to_string()))?;

                return semver::VersionReq::parse(&format!(">={}.{}.0, <{}.0.0", major, minor, major + 1))
                    .map_err(|e| ReleaserError::VersionError(e.to_string()));
            }
            3 => {
                let major: u64 = parts[0].parse()
                    .map_err(|_| ReleaserError::VersionError(constraint.to_string()))?;
                let minor: u64 = parts[1].parse()
                    .map_err(|_| ReleaserError::VersionError(constraint.to_string()))?;

                return semver::VersionReq::parse(&format!(">={}, <{}.{}.0", version, major, minor + 1))
                    .map_err(|e| ReleaserError::VersionError(e.to_string()));
            }
            _ => {}
        }
    }

    // Handle == (exact match)
    let constraint = constraint.replace("==", "=");

    semver::VersionReq::parse(&constraint)
        .map_err(|e| ReleaserError::VersionError(format!("{}: {}", constraint, e)))
}

impl Default for PyPiClient {
    fn default() -> Self {
        Self::new()
    }
}