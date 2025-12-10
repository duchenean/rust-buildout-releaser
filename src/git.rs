use std::process::Command;
use crate::error::{ReleaserError, Result};
use crate::buildout::VersionUpdate;

pub struct GitOps {
    /// Working directory
    work_dir: Option<String>,
}

impl GitOps {
    pub fn new() -> Self {
        Self { work_dir: None }
    }

    pub fn with_work_dir<S: Into<String>>(mut self, dir: S) -> Self {
        self.work_dir = Some(dir.into());
        self
    }

    fn run_git(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("git");

        if let Some(ref dir) = self.work_dir {
            cmd.current_dir(dir);
        }

        let output = cmd.args(args)
            .output()
            .map_err(|e| ReleaserError::GitError(format!("Failed to run git: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ReleaserError::GitError(format!(
                "git {} failed: {}",
                args.join(" "),
                stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check if we're in a git repository
    pub fn is_repo(&self) -> bool {
        self.run_git(&["rev-parse", "--git-dir"]).is_ok()
    }

    /// Get current branch name
    pub fn current_branch(&self) -> Result<String> {
        self.run_git(&["rev-parse", "--abbrev-ref", "HEAD"])
    }

    /// Check if working directory is clean
    pub fn is_clean(&self) -> Result<bool> {
        let status = self.run_git(&["status", "--porcelain"])?;
        Ok(status.is_empty())
    }

    /// Stage a file
    pub fn add(&self, file: &str) -> Result<()> {
        self.run_git(&["add", file])?;
        Ok(())
    }

    /// Create a commit with the given message
    pub fn commit(&self, message: &str) -> Result<()> {
        self.run_git(&["commit", "-m", message])?;
        Ok(())
    }

    /// Create a tag
    pub fn tag(&self, tag_name: &str, message: Option<&str>) -> Result<()> {
        match message {
            Some(msg) => self.run_git(&["tag", "-a", tag_name, "-m", msg])?,
            None => self.run_git(&["tag", tag_name])?,
        };
        Ok(())
    }

    /// Push commits and tags
    pub fn push(&self, include_tags: bool) -> Result<()> {
        self.run_git(&["push"])?;
        if include_tags {
            self.run_git(&["push", "--tags"])?;
        }
        Ok(())
    }

    /// Get the latest tag
    pub fn latest_tag(&self) -> Result<Option<String>> {
        match self.run_git(&["describe", "--tags", "--abbrev=0"]) {
            Ok(tag) => Ok(Some(tag)),
            Err(_) => Ok(None), // No tags exist
        }
    }

    /// Get all tags matching a pattern
    pub fn tags(&self, pattern: Option<&str>) -> Result<Vec<String>> {
        let args = match pattern {
            Some(p) => vec!["tag", "-l", p],
            None => vec!["tag", "-l"],
        };

        let output = self.run_git(&args)?;
        Ok(output.lines().map(|s| s.to_string()).collect())
    }

    /// Get all version tags, sorted by version (descending)
    /// Recognizes tags like: v1.2.3, 1.2.3, v1.2.3-beta, etc.
    pub fn get_version_tags(&self, prefix: &str) -> Result<Vec<(String, crate::version::Version)>> {
        let all_tags = self.tags(None)?;

        let mut version_tags: Vec<(String, crate::version::Version)> = all_tags
            .into_iter()
            .filter_map(|tag| {
                // Remove prefix if present
                let version_str = if prefix.is_empty() {
                    tag.clone()
                } else if tag.starts_with(prefix) {
                    tag[prefix.len()..].to_string()
                } else {
                    return None;
                };

                // Try to parse as version
                crate::version::Version::parse(&version_str)
                    .ok()
                    .map(|v| (tag, v))
            })
            .collect();

        // Sort by version descending (highest first)
        version_tags.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(version_tags)
    }

    /// Get the latest version from git tags
    pub fn get_latest_version(&self, prefix: &str) -> Result<Option<crate::version::Version>> {
        let version_tags = self.get_version_tags(prefix)?;
        Ok(version_tags.into_iter().next().map(|(_, v)| v))
    }

    /// Generate commit message from updates
    pub fn generate_commit_message(updates: &[VersionUpdate], template: &str) -> String {
        let packages_str = updates.iter()
            .map(|u| format!("{} = {}", u.package_name, u.new_version))
            .collect::<Vec<_>>()
            .join(", ");

        let date = chrono_lite_date();

        template
            .replace("{packages}", &packages_str)
            .replace("{date}", &date)
    }
}

impl Default for GitOps {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple date function without pulling in chrono
fn chrono_lite_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap();

    let secs = duration.as_secs();

    // Simple date calculation (approximate, good enough for commit messages)
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let remaining_days = days % 365;
    let months = remaining_days / 30 + 1;
    let day = remaining_days % 30 + 1;

    format!("{:04}-{:02}-{:02}", years, months, day)
}

/// GitHub CLI operations
pub struct GitHubOps;

impl GitHubOps {
    /// Check if gh CLI is available
    pub fn is_available() -> bool {
        Command::new("gh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Check if authenticated
    pub fn is_authenticated() -> Result<bool> {
        let output = Command::new("gh")
            .args(["auth", "status"])
            .output()
            .map_err(|e| ReleaserError::GitError(format!("Failed to run gh: {}", e)))?;

        Ok(output.status.success())
    }

    /// Create a release
    pub fn create_release(
        tag: &str,
        title: Option<&str>,
        notes: Option<&str>,
        draft: bool,
        prerelease: bool,
    ) -> Result<()> {
        let mut args = vec!["release", "create", tag];

        if let Some(t) = title {
            args.push("--title");
            args.push(t);
        }

        if let Some(n) = notes {
            args.push("--notes");
            args.push(n);
        }

        if draft {
            args.push("--draft");
        }

        if prerelease {
            args.push("--prerelease");
        }

        let output = Command::new("gh")
            .args(&args)
            .output()
            .map_err(|e| ReleaserError::GitError(format!("Failed to run gh: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ReleaserError::GitError(format!(
                "gh release create failed: {}",
                stderr
            )));
        }

        Ok(())
    }
}