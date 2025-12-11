use serde::Deserialize;
use crate::error::{ReleaserError, Result};
use semver::{BuildMetadata, Prerelease};

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
        let (req, exclusions) = parse_version_constraint(constraint)?;

        let mut versions: Vec<(semver::Version, String)> = info.releases
            .iter()
            .filter(|(_, releases)| !releases.is_empty() && !releases.iter().all(|r| r.yanked))
            .filter_map(|(version_str, _)| {
                parse_python_version(version_str).map(|v| (v, version_str.clone()))
            })
            .filter(|(v, _)| req.matches(v))
            .filter(|(v, _)| {
                exclusions
                    .iter()
                    .all(|(start, end)| !(v >= start && v < end))
            })
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
    let re = regex::Regex::new(concat!(
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
fn parse_version_constraint(constraint: &str) -> Result<(semver::VersionReq, Vec<(semver::Version, semver::Version)>)> {
    // Convert Python-style constraints to semver
    // ~=X.Y -> >=X.Y.0, <X+1.0.0 (approximately)
    // ==X.Y.Z -> =X.Y.Z
    // >=X.Y.Z -> >=X.Y.Z
    // etc.

    let constraint = constraint.trim();

    if constraint.is_empty() {
        return Err(ReleaserError::VersionError("Empty version constraint".to_string()));
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

        let release_len = regex::Regex::new(r"^(\d+(?:\.\d+)*)")
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

fn normalize_constraint_part(part: &str) -> Result<(String, Vec<(semver::Version, semver::Version)>)> {
    let part = part.trim();

    if part.is_empty() {
        return Err(ReleaserError::VersionError("Empty constraint segment".to_string()));
    }

    // Wildcard equality (==1.2.*)
    let wildcard_re = regex::Regex::new(r"^(==|!=)\s*(\d+)(?:\.(\d+))?\.\*$")
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

    let re = regex::Regex::new(r"^(<=|>=|==|===|!=|<|>|=)?\s*(.+)$")
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

impl Default for PyPiClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_constraint_part, parse_python_version, parse_version_constraint};

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

        let v = parse_python_version("2.5").expect("should parse two-segment release");
        assert_eq!(v.to_string(), "2.5.0");

        let v = parse_python_version("7").expect("should parse single-segment release");
        assert_eq!(v.to_string(), "7.0.0");
    }

    #[test]
    fn parses_wildcard_constraints() {
        let (req, exclusions) = parse_version_constraint("==3.8.*").expect("should parse wildcard equality");
        let matches = req.matches(&semver::Version::parse("3.8.5").unwrap());
        assert!(matches, "should accept version within wildcard range");
        assert!(exclusions.is_empty());

        let (req, exclusions) = parse_version_constraint("!=2.*").expect("should parse wildcard inequality");
        assert!(req.matches(&semver::Version::parse("1.9.9").unwrap()));
        assert_eq!(exclusions.len(), 1);
        let (lower, upper) = &exclusions[0];
        assert_eq!(lower, &semver::Version::new(2, 0, 0));
        assert_eq!(upper, &semver::Version::new(3, 0, 0));
    }

    #[test]
    fn parses_partial_comparators() {
        let (req, exclusions) = parse_version_constraint(">=3.8").expect("should parse partial comparator");
        assert!(req.matches(&semver::Version::parse("3.8.1").unwrap()));
        assert!(exclusions.is_empty());

        let (req, exclusions) = parse_version_constraint("~=3.8").expect("should parse compatible release");
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