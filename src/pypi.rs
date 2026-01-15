use crate::error::{ReleaserError, Result};
use crate::version::python::{parse_python_version, parse_version_constraint};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;

const USER_AGENT: &str = concat!("bldr/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RETRIES: usize = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(300);

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

#[derive(Clone)]
pub struct PyPiClient {
    client: reqwest::Client,
    base_url: String,
}

impl PyPiClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            client,
            base_url: "https://pypi.org/pypi".to_string(),
        })
    }

    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response> {
        let mut last_error: Option<ReleaserError> = None;

        for attempt in 0..MAX_RETRIES {
            match self.client.get(url).send().await {
                Ok(response) => {
                    if response.status().is_server_error() {
                        last_error = Some(ReleaserError::PyPiError(format!(
                            "HTTP {} for {}",
                            response.status(),
                            url
                        )));
                    } else {
                        return Ok(response);
                    }
                }
                Err(err) => {
                    last_error = Some(ReleaserError::HttpError(err));
                }
            }

            if attempt + 1 < MAX_RETRIES {
                let delay = RETRY_BACKOFF * 2u32.pow(attempt as u32);
                sleep(delay).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ReleaserError::PyPiError("Failed to contact PyPI after retries".to_string())
        }))
    }

    /// Fetch package information from PyPI
    pub async fn get_package_info(&self, package_name: &str) -> Result<PyPiPackageInfo> {
        let url = format!("{}/{}/json", self.base_url, package_name);

        let response = self.get_with_retry(&url).await?;

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

        response
            .json::<PyPiPackageInfo>()
            .await
            .map_err(|e| ReleaserError::PyPiError(format!("Failed to parse response: {}", e)))
    }

    /// Get the latest version of a package
    pub async fn get_latest_version(
        &self,
        package_name: &str,
        allow_prerelease: bool,
    ) -> Result<VersionInfo> {
        let info = self.get_package_info(package_name).await?;

        // Get all non-yanked versions
        let mut versions: Vec<(semver::Version, String)> = info
            .releases
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

        let (parsed_version, version_str) = versions.into_iter().next().ok_or_else(|| {
            ReleaserError::PyPiError(format!("No valid versions found for {}", package_name))
        })?;

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

        let mut versions: Vec<(semver::Version, String)> = info
            .releases
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

        let (parsed_version, version_str) = versions.into_iter().next().ok_or_else(|| {
            ReleaserError::PyPiError(format!(
                "No versions matching '{}' for {}",
                constraint, package_name
            ))
        })?;

        Ok(VersionInfo {
            package_name: info.info.name,
            version: version_str,
            is_prerelease: !parsed_version.pre.is_empty(),
        })
    }
}
