use regex::Regex;
use reqwest::Client;
use std::path::Path;
use crate::buildout::VersionUpdate;
use crate::config::{ChangelogConfig, ChangelogFormat, PackageConfig};
use crate::error::{ReleaserError, Result};

#[derive(Debug, Clone)]
pub struct PackageChangelog {
    pub package_name: String,
    pub old_version: String,
    pub new_version: String,
    pub entries: Vec<ChangelogEntry>,
    pub raw_content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub version: String,
    pub date: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ConsolidatedChangelog {
    pub release_version: String,
    pub date: String,
    pub package_changelogs: Vec<PackageChangelog>,
    pub header_template: String,
    pub package_template: String,
}

pub struct ChangelogCollector {
    client: Client,
    changelog_files: Vec<String>,
    github_branches: Vec<String>,
}

impl ChangelogCollector {
    pub fn new() -> Self {
        Self::with_config(&ChangelogConfig::default())
    }

    pub fn with_config(config: &ChangelogConfig) -> Self {
        let mut github_branches = vec!["main".to_string(), "master".to_string()];
        github_branches.extend(config.github_branches.clone());

        Self {
            client: Client::builder()
                .user_agent("buildout-releaser/0.1.0")
                .build()
                .expect("Failed to create HTTP client"),
            changelog_files: config.changelog_files.clone(),
            github_branches,
        }
    }

    /// Fetch changelog for a package from various sources
    pub async fn fetch_changelog(
        &self,
        package_name: &str,
        old_version: &str,
        new_version: &str,
        custom_url: Option<&str>,
    ) -> Result<PackageChangelog> {
        // Try custom URL first if provided
        let raw_content = if let Some(url) = custom_url {
            self.fetch_url_content(url).await.ok().flatten()
        } else {
            self.try_fetch_from_pypi(package_name).await.ok().flatten()
        };

        let entries = if let Some(ref content) = raw_content {
            self.parse_changelog(content, old_version, new_version)
        } else {
            Vec::new()
        };

        Ok(PackageChangelog {
            package_name: package_name.to_string(),
            old_version: old_version.to_string(),
            new_version: new_version.to_string(),
            entries,
            raw_content,
        })
    }

    /// Try to fetch changelog from PyPI package description or project URLs
    async fn try_fetch_from_pypi(&self, package_name: &str) -> Result<Option<String>> {
        let url = format!("https://pypi.org/pypi/{}/json", package_name);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            ReleaserError::PyPiError(format!("Failed to parse PyPI response: {}", e))
        })?;

        // Try to get changelog from description
        if let Some(description) = data["info"]["description"].as_str() {
            if Self::looks_like_changelog(description) {
                return Ok(Some(description.to_string()));
            }
        }

        // Try to fetch from project URLs (CHANGES.txt, CHANGELOG.md, etc.)
        if let Some(urls) = data["info"]["project_urls"].as_object() {
            for key in ["Changelog", "Changes", "History", "Release Notes"] {
                if let Some(changelog_url) = urls.get(key).and_then(|v| v.as_str()) {
                    if let Ok(Some(content)) = self.fetch_url_content(changelog_url).await {
                        return Ok(Some(content));
                    }
                }
            }
        }

        // Try common GitHub raw URLs if we have a GitHub project URL
        if let Some(urls) = data["info"]["project_urls"].as_object() {
            for key in ["Homepage", "Source", "Repository", "GitHub"] {
                if let Some(url) = urls.get(key).and_then(|v| v.as_str()) {
                    if url.contains("github.com") {
                        if let Ok(Some(content)) = self.try_github_changelog(url).await {
                            return Ok(Some(content));
                        }
                    }
                }
            }
        }

        // Also check home_page
        if let Some(home_page) = data["info"]["home_page"].as_str() {
            if home_page.contains("github.com") {
                if let Ok(Some(content)) = self.try_github_changelog(home_page).await {
                    return Ok(Some(content));
                }
            }
        }

        Ok(None)
    }

    /// Check if content looks like a changelog
    fn looks_like_changelog(content: &str) -> bool {
        let lower = content.to_lowercase();
        lower.contains("changelog")
            || lower.contains("changes")
            || lower.contains("history")
            || lower.contains("release notes")
            || Regex::new(r"(?i)##?\s*\[?\d+\.\d+").unwrap().is_match(content)
    }

    /// Fetch content from a URL
    async fn fetch_url_content(&self, url: &str) -> Result<Option<String>> {
        let response = self.client.get(url).send().await?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let content = response.text().await?;
        Ok(Some(content))
    }

    /// Try to fetch changelog from GitHub repository
    async fn try_github_changelog(&self, github_url: &str) -> Result<Option<String>> {
        // Convert GitHub URL to raw content URL
        let repo_pattern = Regex::new(r"github\.com/([^/]+)/([^/]+)").unwrap();

        let (owner, repo) = if let Some(caps) = repo_pattern.captures(github_url) {
            (
                caps.get(1).unwrap().as_str(),
                caps.get(2).unwrap().as_str().trim_end_matches(".git"),
            )
        } else {
            return Ok(None);
        };

        // Try configured changelog files and branches
        for branch in &self.github_branches {
            for file in &self.changelog_files {
                let raw_url = format!(
                    "https://raw.githubusercontent.com/{}/{}/{}/{}",
                    owner, repo, branch, file
                );

                if let Ok(Some(content)) = self.fetch_url_content(&raw_url).await {
                    return Ok(Some(content));
                }
            }
        }

        Ok(None)
    }

    /// Parse changelog content and extract entries between versions
    fn parse_changelog(
        &self,
        content: &str,
        old_version: &str,
        new_version: &str,
    ) -> Vec<ChangelogEntry> {
        // Try different changelog formats
        if let Some(parsed) = self.try_parse_markdown_changelog(content, old_version, new_version) {
            return parsed;
        }

        if let Some(parsed) = self.try_parse_rst_changelog(content, old_version, new_version) {
            return parsed;
        }

        if let Some(parsed) = self.try_parse_generic_changelog(content, old_version, new_version) {
            return parsed;
        }

        Vec::new()
    }

    /// Parse Markdown-style changelog (## [version] or ## version)
    fn try_parse_markdown_changelog(
        &self,
        content: &str,
        old_version: &str,
        new_version: &str,
    ) -> Option<Vec<ChangelogEntry>> {
        let header_pattern =
            Regex::new(r"(?m)^##\s+\[?v?(\d+\.\d+(?:\.\d+)?(?:[._-]?\w+)?)\]?(?:\s*[-–—]\s*(.+))?$")
                .ok()?;

        let mut entries = Vec::new();
        let mut capture_content = false;
        let mut current_entry: Option<ChangelogEntry> = None;
        let mut content_buffer = String::new();

        let old_ver_normalized = normalize_version(old_version);
        let new_ver_normalized = normalize_version(new_version);

        for line in content.lines() {
            if let Some(caps) = header_pattern.captures(line) {
                if let Some(mut entry) = current_entry.take() {
                    entry.content = content_buffer.trim().to_string();
                    if !entry.content.is_empty() {
                        entries.push(entry);
                    }
                    content_buffer.clear();
                }

                let version = caps.get(1).unwrap().as_str();
                let date = caps.get(2).map(|m| m.as_str().trim().to_string());
                let ver_normalized = normalize_version(version);

                if compare_versions(&ver_normalized, &old_ver_normalized) > 0
                    && compare_versions(&ver_normalized, &new_ver_normalized) <= 0
                {
                    capture_content = true;
                    current_entry = Some(ChangelogEntry {
                        version: version.to_string(),
                        date,
                        content: String::new(),
                    });
                } else if compare_versions(&ver_normalized, &old_ver_normalized) <= 0 {
                    capture_content = false;
                }
            } else if capture_content {
                content_buffer.push_str(line);
                content_buffer.push('\n');
            }
        }

        if let Some(mut entry) = current_entry.take() {
            entry.content = content_buffer.trim().to_string();
            if !entry.content.is_empty() {
                entries.push(entry);
            }
        }

        if entries.is_empty() {
            None
        } else {
            Some(entries)
        }
    }

    /// Parse RST-style changelog
    fn try_parse_rst_changelog(
        &self,
        content: &str,
        old_version: &str,
        new_version: &str,
    ) -> Option<Vec<ChangelogEntry>> {
        let header_pattern =
            Regex::new(r"(?m)^v?(\d+\.\d+(?:\.\d+)?(?:[._-]?\w+)?)\s*(?:\(([^)]+)\))?\s*$")
                .ok()?;
        let underline_pattern = Regex::new(r"^[-=~^]+$").ok()?;

        let lines: Vec<&str> = content.lines().collect();
        let mut entries = Vec::new();
        let mut capture_content = false;
        let mut current_entry: Option<ChangelogEntry> = None;
        let mut content_buffer = String::new();

        let old_ver_normalized = normalize_version(old_version);
        let new_ver_normalized = normalize_version(new_version);

        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];

            if let Some(caps) = header_pattern.captures(line) {
                let has_underline = i + 1 < lines.len() && underline_pattern.is_match(lines[i + 1]);

                if has_underline {
                    if let Some(mut entry) = current_entry.take() {
                        entry.content = content_buffer.trim().to_string();
                        if !entry.content.is_empty() {
                            entries.push(entry);
                        }
                        content_buffer.clear();
                    }

                    let version = caps.get(1).unwrap().as_str();
                    let date = caps.get(2).map(|m| m.as_str().trim().to_string());
                    let ver_normalized = normalize_version(version);

                    if compare_versions(&ver_normalized, &old_ver_normalized) > 0
                        && compare_versions(&ver_normalized, &new_ver_normalized) <= 0
                    {
                        capture_content = true;
                        current_entry = Some(ChangelogEntry {
                            version: version.to_string(),
                            date,
                            content: String::new(),
                        });
                    } else if compare_versions(&ver_normalized, &old_ver_normalized) <= 0 {
                        capture_content = false;
                    }

                    i += 2;
                    continue;
                }
            }

            if capture_content && !underline_pattern.is_match(line) {
                content_buffer.push_str(line);
                content_buffer.push('\n');
            }

            i += 1;
        }

        if let Some(mut entry) = current_entry.take() {
            entry.content = content_buffer.trim().to_string();
            if !entry.content.is_empty() {
                entries.push(entry);
            }
        }

        if entries.is_empty() {
            None
        } else {
            Some(entries)
        }
    }

    /// Generic changelog parser for other formats
    fn try_parse_generic_changelog(
        &self,
        content: &str,
        old_version: &str,
        new_version: &str,
    ) -> Option<Vec<ChangelogEntry>> {
        let header_pattern = Regex::new(
            r"(?m)^(?:\*\s+|Version\s+|v)?(\d+\.\d+(?:\.\d+)?(?:[._-]?\w+)?)(?:\s*[-:]\s*(.+))?$",
        )
            .ok()?;

        let mut entries = Vec::new();
        let mut capture_content = false;
        let mut current_entry: Option<ChangelogEntry> = None;
        let mut content_buffer = String::new();

        let old_ver_normalized = normalize_version(old_version);
        let new_ver_normalized = normalize_version(new_version);

        for line in content.lines() {
            if let Some(caps) = header_pattern.captures(line) {
                let version = caps.get(1).unwrap().as_str();

                if !version.contains('.') {
                    if capture_content {
                        content_buffer.push_str(line);
                        content_buffer.push('\n');
                    }
                    continue;
                }

                if let Some(mut entry) = current_entry.take() {
                    entry.content = content_buffer.trim().to_string();
                    if !entry.content.is_empty() {
                        entries.push(entry);
                    }
                    content_buffer.clear();
                }

                let date = caps.get(2).map(|m| m.as_str().trim().to_string());
                let ver_normalized = normalize_version(version);

                if compare_versions(&ver_normalized, &old_ver_normalized) > 0
                    && compare_versions(&ver_normalized, &new_ver_normalized) <= 0
                {
                    capture_content = true;
                    current_entry = Some(ChangelogEntry {
                        version: version.to_string(),
                        date,
                        content: String::new(),
                    });
                } else if compare_versions(&ver_normalized, &old_ver_normalized) <= 0 {
                    capture_content = false;
                }
            } else if capture_content {
                content_buffer.push_str(line);
                content_buffer.push('\n');
            }
        }

        if let Some(mut entry) = current_entry.take() {
            entry.content = content_buffer.trim().to_string();
            if !entry.content.is_empty() {
                entries.push(entry);
            }
        }

        if entries.is_empty() {
            None
        } else {
            Some(entries)
        }
    }

    /// Collect changelogs for multiple package updates
    pub async fn collect_changelogs(
        &self,
        updates: &[VersionUpdate],
        package_configs: &[PackageConfig],
    ) -> Result<Vec<PackageChangelog>> {
        let mut changelogs = Vec::new();

        for update in updates {
            // Find the package config to get custom changelog URL
            let custom_url = package_configs
                .iter()
                .find(|p| p.name == update.package_name || p.buildout_name() == update.package_name)
                .and_then(|p| p.changelog_url.as_deref());

            match self
                .fetch_changelog(
                    &update.package_name,
                    &update.old_version,
                    &update.new_version,
                    custom_url,
                )
                .await
            {
                Ok(changelog) => changelogs.push(changelog),
                Err(e) => {
                    eprintln!(
                        "Warning: Could not fetch changelog for {}: {}",
                        update.package_name, e
                    );
                    changelogs.push(PackageChangelog {
                        package_name: update.package_name.clone(),
                        old_version: update.old_version.clone(),
                        new_version: update.new_version.clone(),
                        entries: Vec::new(),
                        raw_content: None,
                    });
                }
            }
        }

        Ok(changelogs)
    }
}

impl Default for ChangelogCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsolidatedChangelog {
    /// Create a consolidated changelog from multiple package changelogs
    pub fn new(
        release_version: &str,
        date: &str,
        package_changelogs: Vec<PackageChangelog>,
    ) -> Self {
        Self::with_templates(
            release_version,
            date,
            package_changelogs,
            &ChangelogConfig::default(),
        )
    }

    pub fn with_templates(
        release_version: &str,
        date: &str,
        package_changelogs: Vec<PackageChangelog>,
        config: &ChangelogConfig,
    ) -> Self {
        Self {
            release_version: release_version.to_string(),
            date: date.to_string(),
            package_changelogs,
            header_template: config.header_template.clone(),
            package_template: config.package_template.clone(),
        }
    }

    /// Render as Markdown
    pub fn to_markdown(&self) -> String {
        let mut output = String::new();

        // Apply header template
        let header = self
            .header_template
            .replace("{version}", &self.release_version)
            .replace("{date}", &self.date);
        output.push_str(&header);
        output.push_str("\n\n");

        for pkg in &self.package_changelogs {
            // Apply package template
            let pkg_header = self
                .package_template
                .replace("{package}", &pkg.package_name)
                .replace("{old_version}", &pkg.old_version)
                .replace("{new_version}", &pkg.new_version);
            output.push_str(&pkg_header);
            output.push_str("\n\n");

            if pkg.entries.is_empty() {
                output.push_str("*No changelog entries found.*\n\n");
            } else {
                for entry in &pkg.entries {
                    let date_str = entry
                        .date
                        .as_ref()
                        .map(|d| format!(" ({})", d))
                        .unwrap_or_default();

                    output.push_str(&format!("#### Version {}{}\n\n", entry.version, date_str));
                    output.push_str(&entry.content);
                    output.push_str("\n\n");
                }
            }
        }

        output
    }

    /// Render as RST (reStructuredText)
    pub fn to_rst(&self) -> String {
        let mut output = String::new();

        let title = format!("Release {}", self.release_version);
        output.push_str(&title);
        output.push('\n');
        output.push_str(&"=".repeat(title.len()));
        output.push_str("\n\n");

        output.push_str(&format!("**Date:** {}\n\n", self.date));

        output.push_str("Package Updates\n");
        output.push_str("---------------\n\n");

        for pkg in &self.package_changelogs {
            let pkg_title = format!(
                "{} ({} → {})",
                pkg.package_name, pkg.old_version, pkg.new_version
            );
            output.push_str(&pkg_title);
            output.push('\n');
            output.push_str(&"~".repeat(pkg_title.len()));
            output.push_str("\n\n");

            if pkg.entries.is_empty() {
                output.push_str("*No changelog entries found.*\n\n");
            } else {
                for entry in &pkg.entries {
                    let date_str = entry
                        .date
                        .as_ref()
                        .map(|d| format!(" ({})", d))
                        .unwrap_or_default();

                    let ver_title = format!("Version {}{}", entry.version, date_str);
                    output.push_str(&ver_title);
                    output.push('\n');
                    output.push_str(&"^".repeat(ver_title.len()));
                    output.push_str("\n\n");
                    output.push_str(&entry.content);
                    output.push_str("\n\n");
                }
            }
        }

        output
    }

    /// Render as plain text
    pub fn to_text(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "RELEASE {} ({})\n",
            self.release_version, self.date
        ));
        output.push_str(&"=".repeat(60));
        output.push_str("\n\n");

        for pkg in &self.package_changelogs {
            output.push_str(&format!(
                "{}: {} → {}\n",
                pkg.package_name, pkg.old_version, pkg.new_version
            ));
            output.push_str(&"-".repeat(40));
            output.push('\n');

            if pkg.entries.is_empty() {
                output.push_str("  No changelog entries found.\n");
            } else {
                for entry in &pkg.entries {
                    let date_str = entry
                        .date
                        .as_ref()
                        .map(|d| format!(" ({})", d))
                        .unwrap_or_default();

                    output.push_str(&format!("\n  Version {}{}:\n", entry.version, date_str));
                    for line in entry.content.lines() {
                        output.push_str(&format!("    {}\n", line));
                    }
                }
            }
            output.push('\n');
        }

        output
    }

    /// Render in specified format
    pub fn render(&self, format: ChangelogFormat) -> String {
        match format {
            ChangelogFormat::Markdown => self.to_markdown(),
            ChangelogFormat::Rst => self.to_rst(),
            ChangelogFormat::Text => self.to_text(),
        }
    }

    /// Save changelog to file, prepending to existing content
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P, format: ChangelogFormat) -> Result<()> {
        let new_content = self.render(format);
        let path = path.as_ref();

        if path.exists() {
            // Read existing content
            let existing_content = std::fs::read_to_string(path)?;

            // Prepend new content to existing
            let combined = Self::prepend_to_changelog(&new_content, &existing_content, format);
            std::fs::write(path, combined)?;
        } else {
            // Create new file with header
            let with_header = Self::add_file_header(&new_content, format);
            std::fs::write(path, with_header)?;
        }

        Ok(())
    }

    /// Prepend new changelog entry to existing content
    fn prepend_to_changelog(new_content: &str, existing_content: &str, format: ChangelogFormat) -> String {
        match format {
            ChangelogFormat::Markdown => {
                // Check if file has a main title (# Changelog or similar)
                let lines: Vec<&str> = existing_content.lines().collect();

                // Find where the first release entry starts (## ...)
                let mut insert_position = 0;
                let mut found_main_title = false;

                for (i, line) in lines.iter().enumerate() {
                    let trimmed = line.trim();

                    // Skip empty lines at the beginning
                    if trimmed.is_empty() && !found_main_title {
                        insert_position = i + 1;
                        continue;
                    }

                    // Found main title (# Changelog)
                    if trimmed.starts_with("# ") && !trimmed.starts_with("## ") {
                        found_main_title = true;
                        insert_position = i + 1;
                        continue;
                    }

                    // Skip empty lines after main title
                    if found_main_title && trimmed.is_empty() {
                        insert_position = i + 1;
                        continue;
                    }

                    // Found first release entry or other content
                    if found_main_title || trimmed.starts_with("## ") || trimmed.starts_with("# Release") {
                        break;
                    }

                    insert_position = i + 1;
                }

                // Build the combined content
                let mut result = String::new();

                // Add everything before insertion point
                for line in &lines[..insert_position] {
                    result.push_str(line);
                    result.push('\n');
                }

                // Add new content
                result.push_str(new_content.trim());
                result.push_str("\n\n");

                // Add remaining content
                if insert_position < lines.len() {
                    for line in &lines[insert_position..] {
                        result.push_str(line);
                        result.push('\n');
                    }
                }

                result
            }
            ChangelogFormat::Rst => {
                // Similar logic for RST
                let lines: Vec<&str> = existing_content.lines().collect();
                let mut insert_position = 0;
                let mut found_main_title = false;
                let mut skip_underline = false;

                for (i, line) in lines.iter().enumerate() {
                    let trimmed = line.trim();

                    if skip_underline {
                        skip_underline = false;
                        insert_position = i + 1;
                        continue;
                    }

                    if trimmed.is_empty() && !found_main_title {
                        insert_position = i + 1;
                        continue;
                    }

                    // Check for RST title (followed by === underline)
                    if !found_main_title && i + 1 < lines.len() {
                        let next_line = lines[i + 1].trim();
                        if next_line.chars().all(|c| c == '=') && !next_line.is_empty() {
                            found_main_title = true;
                            skip_underline = true;
                            insert_position = i + 2;
                            continue;
                        }
                    }

                    if found_main_title && trimmed.is_empty() {
                        insert_position = i + 1;
                        continue;
                    }

                    if found_main_title {
                        break;
                    }

                    insert_position = i + 1;
                }

                let mut result = String::new();

                for line in &lines[..insert_position] {
                    result.push_str(line);
                    result.push('\n');
                }

                result.push_str(new_content.trim());
                result.push_str("\n\n");

                if insert_position < lines.len() {
                    for line in &lines[insert_position..] {
                        result.push_str(line);
                        result.push('\n');
                    }
                }

                result
            }
            ChangelogFormat::Text => {
                // For plain text, just prepend with a separator
                format!("{}\n{}\n{}", new_content.trim(), "=".repeat(60), existing_content)
            }
        }
    }

    /// Add a file header for new changelog files
    fn add_file_header(content: &str, format: ChangelogFormat) -> String {
        match format {
            ChangelogFormat::Markdown => {
                format!("# Changelog\n\n{}", content)
            }
            ChangelogFormat::Rst => {
                let title = "Changelog";
                format!(
                    "{}\n{}\n\n{}",
                    title,
                    "=".repeat(title.len()),
                    content
                )
            }
            ChangelogFormat::Text => {
                format!(
                    "CHANGELOG\n{}\n\n{}",
                    "=".repeat(60),
                    content
                )
            }
        }
    }
}

/// Normalize version string for comparison
fn normalize_version(version: &str) -> Vec<u32> {
    version
        .trim_start_matches('v')
        .split(|c: char| c == '.' || c == '-' || c == '_')
        .filter_map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
        .collect()
}

/// Compare two normalized versions
fn compare_versions(a: &[u32], b: &[u32]) -> i32 {
    let max_len = a.len().max(b.len());

    for i in 0..max_len {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);

        if av < bv {
            return -1;
        } else if av > bv {
            return 1;
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("1.2.3"), vec![1, 2, 3]);
        assert_eq!(normalize_version("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(normalize_version("1.2.3a1"), vec![1, 2, 3, 1]);
        assert_eq!(normalize_version("1.2"), vec![1, 2]);
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions(&vec![1, 2, 3], &vec![1, 2, 3]), 0);
        assert_eq!(compare_versions(&vec![1, 2, 3], &vec![1, 2, 4]), -1);
        assert_eq!(compare_versions(&vec![1, 2, 4], &vec![1, 2, 3]), 1);
        assert_eq!(compare_versions(&vec![1, 2], &vec![1, 2, 0]), 0);
        assert_eq!(compare_versions(&vec![2, 0, 0], &vec![1, 9, 9]), 1);
    }

    #[test]
    fn test_prepend_to_markdown_changelog() {
        let existing = r#"# Changelog

## Release 1.0.0

**Date:** 2024-01-01

- Initial release
"#;

        let new_entry = r#"## Release 1.1.0

**Date:** 2024-02-01

- New feature
"#;

        let result = ConsolidatedChangelog::prepend_to_changelog(
            new_entry,
            existing,
            ChangelogFormat::Markdown,
        );

        // New entry should be after the header but before the old release
        assert!(result.contains("# Changelog"));
        assert!(result.find("## Release 1.1.0").unwrap() < result.find("## Release 1.0.0").unwrap());
    }

    #[test]
    fn test_add_file_header_markdown() {
        let content = "## Release 1.0.0\n\n- Initial release\n";
        let result = ConsolidatedChangelog::add_file_header(content, ChangelogFormat::Markdown);

        assert!(result.starts_with("# Changelog"));
        assert!(result.contains("All notable changes"));
        assert!(result.contains("## Release 1.0.0"));
    }
}