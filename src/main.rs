mod buildout;
mod changelog;
mod cli;
mod config;
mod error;
mod git;
mod pypi;
mod version;

use clap::{CommandFactory, Parser};
use colored::*;
use dialoguer::{Confirm, MultiSelect};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

use buildout::{BuildoutVersions, VersionUpdate};
use changelog::{ChangelogCollector, ConsolidatedChangelog};
use cli::{Cli, CliChangelogFormat, Commands};
use config::{ChangelogFormat, Config, PackageConfig};
use error::{ReleaserError, Result};
use git::{GitHubOps, GitOps};
use pypi::PyPiClient;
use version::{MetadataUpdater, Version, VersionManager};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Completions { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "bldr", &mut std::io::stdout());
            Ok(())
        }
        Commands::Init { force } => cmd_init(&cli.config, force),
        Commands::Check { packages, json } => {
            cmd_check(&cli.config, packages, json, cli.verbose).await
        }
        Commands::Update {
            packages,
            yes,
            dry_run,
            commit,
            push,
        } => {
            cmd_update(
                &cli.config,
                packages,
                yes,
                dry_run,
                commit,
                push,
                cli.non_interactive,
                cli.verbose,
            )
            .await
        }
        Commands::Release {
            tag,
            bump,
            message,
            no_push,
            no_github,
            draft,
            no_metadata,
        } => cmd_release(
            &cli.config,
            tag,
            bump,
            message.as_deref(),
            no_push,
            no_github,
            draft,
            no_metadata,
            cli.non_interactive,
            cli.verbose,
        ),
        Commands::UpdateRelease {
            tag,
            bump,
            packages,
            yes,
            message,
            no_push,
            no_github,
            draft,
            dry_run,
            changelog,
            no_changelog,
            changelog_format,
            changelog_file,
            no_metadata,
        } => {
            cmd_update_release(
                &cli.config,
                tag,
                bump,
                packages,
                yes,
                message,
                no_push,
                no_github,
                draft,
                dry_run,
                changelog,
                no_changelog,
                changelog_format,
                changelog_file,
                no_metadata,
                cli.non_interactive,
                cli.verbose,
            )
            .await
        }
        Commands::Changelog {
            packages,
            format,
            output,
            stdout,
            release_version,
            rebuild,
        } => {
            cmd_changelog(
                &cli.config,
                packages,
                format,
                output,
                stdout,
                release_version,
                rebuild,
                cli.verbose,
            )
            .await
        }
        Commands::Version { bump, list_levels } => {
            cmd_version(&cli.config, bump, list_levels, cli.verbose)
        }
        Commands::Add {
            package,
            constraint,
            buildout_name,
            changelog_url,
        } => cmd_add(
            &cli.config,
            &package,
            constraint,
            buildout_name,
            changelog_url,
        ),
        Commands::Remove { package } => cmd_remove(&cli.config, &package),
        Commands::List { detailed } => cmd_list(&cli.config, detailed).await,
        Commands::Info { package, versions } => cmd_info(&package, versions).await,
    }
}

// ============================================================================
// Command Implementations
// ============================================================================

fn cmd_init(config_path: &str, force: bool) -> Result<()> {
    let path = std::path::Path::new(config_path);

    if path.exists() && !force {
        return Err(ReleaserError::ConfigError(format!(
            "Config file '{}' already exists. Use --force to overwrite.",
            config_path
        )));
    }

    Config::create_default(path)?;
    println!("{} Created config file: {}", "✓".green(), config_path);
    println!("  Edit this file to configure your packages and settings.");

    Ok(())
}

async fn rebuild_changelog_from_tags(
    config: &Config,
    packages_to_check: &[PackageConfig],
    format: ChangelogFormat,
    output_file: Option<String>,
    verbose: bool,
) -> Result<()> {
    let git = GitOps::new();

    if !git.is_repo() {
        return Err(ReleaserError::GitError(
            "Rebuild requires running inside a git repository".to_string(),
        ));
    }

    let mut version_tags = git.get_version_tags(&config.github.tag_prefix)?;

    if version_tags.len() < 2 {
        return Err(ReleaserError::GitError(
            "Need at least two version tags to rebuild changelog".to_string(),
        ));
    }

    // Sort ascending (oldest first) for a full rebuild
    version_tags.reverse();

    let versions_file = &config.versions_file;
    let mut snapshots = Vec::new();

    for (tag, _) in &version_tags {
        if verbose {
            println!("Loading versions from tag {}...", tag);
        }

        let content = git.show_file_at_ref(tag, versions_file)?;
        snapshots.push(BuildoutVersions::from_content(
            content,
            format!("{}@{}", versions_file, tag),
        )?);
    }

    let collector = ChangelogCollector::with_config(&config.changelog);
    let mut rendered_entries = Vec::new();

    for window in snapshots.windows(2).zip(version_tags.windows(2)) {
        let (versions_pair, tag_pair) = window;
        let previous = &versions_pair[0];
        let current = &versions_pair[1];

        let current_tag = &tag_pair[1].0;
        let release_version = if config.github.tag_prefix.is_empty() {
            current_tag.clone()
        } else {
            current_tag
                .strip_prefix(&config.github.tag_prefix)
                .unwrap_or(current_tag)
                .to_string()
        };

        let mut updates = Vec::new();

        for pkg in packages_to_check {
            let name = pkg.buildout_name();
            let old_version = previous.get_version(name);
            let new_version = current.get_version(name);

            if let (Some(old_version), Some(new_version)) = (old_version, new_version) {
                if old_version != new_version {
                    updates.push(VersionUpdate {
                        package_name: name.to_string(),
                        old_version: old_version.to_string(),
                        new_version: new_version.to_string(),
                    });
                }
            }
        }

        if updates.is_empty() {
            continue;
        }

        if verbose {
            println!(
                "Generating changelog for {} ({} updates)...",
                current_tag,
                updates.len()
            );
        }

        let changelogs = collector
            .collect_changelogs(&updates, &config.packages)
            .await?;

        let date = git.tag_date(current_tag).unwrap_or_else(|_| current_date());

        let consolidated = ConsolidatedChangelog::with_templates(
            &release_version,
            &date,
            changelogs,
            &config.changelog,
        );

        rendered_entries.push(consolidated.render(format));
    }

    if rendered_entries.is_empty() {
        println!("{}", "No changelog entries generated from tags.".yellow());
        return Ok(());
    }

    let combined_output = combine_rendered_changelog_entries(rendered_entries);

    match output_file {
        Some(path) => {
            std::fs::write(&path, combined_output.trim_end())?;
            println!("\n{} Rebuilt changelog saved to: {}", "✓".green(), path);
        }
        None => {
            println!("\n{}", "═".repeat(60));
            println!("{}", combined_output.trim_end());
        }
    }

    Ok(())
}

fn combine_rendered_changelog_entries(entries: Vec<String>) -> String {
    entries
        .into_iter()
        .rev()
        .map(|entry| entry.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::combine_rendered_changelog_entries;

    #[test]
    fn combines_entries_with_newest_first() {
        let entries = vec![
            "## 1.0.0\n\n- Initial release\n".to_string(),
            "## 1.1.0\n\n- Bug fixes\n".to_string(),
        ];

        let combined = combine_rendered_changelog_entries(entries);

        assert!(combined.starts_with("## 1.1.0"));
        assert!(combined.contains("## 1.0.0"));
        assert!(combined.find("## 1.1.0").unwrap() < combined.find("## 1.0.0").unwrap());
    }

    #[test]
    fn trims_trailing_whitespace_when_combining() {
        let entries = vec![
            "## 2.0.0\n\n- Major updates\n\n".to_string(),
            "## 2.1.0\n\n- Improvements\n\n\n".to_string(),
        ];

        let combined = combine_rendered_changelog_entries(entries);

        assert_eq!(
            combined,
            "## 2.1.0\n\n- Improvements\n\n## 2.0.0\n\n- Major updates"
        );
    }
}

async fn cmd_check(
    config_path: &str,
    packages_filter: Option<String>,
    json_output: bool,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;
    let pypi = PyPiClient::new()?;
    let buildout = BuildoutVersions::load(&config.versions_file)?;

    let packages_to_check = filter_packages(&config.packages, packages_filter.as_deref());

    let progress = if !json_output {
        create_progress_bar(packages_to_check.len(), "Checking packages")
    } else {
        None
    };

    let mut updates = Vec::new();

    for pkg_config in &packages_to_check {
        if let Some(pb) = progress.as_ref() {
            pb.set_message(format!("Checking {}...", pkg_config.name));
            if verbose {
                pb.println(format!("Checking {}...", pkg_config.name));
            }
        } else if verbose {
            println!("Checking {}...", pkg_config.name);
        }

        let latest = match &pkg_config.version_constraint {
            Some(constraint) => {
                pypi.get_matching_version(&pkg_config.name, constraint, pkg_config.allow_prerelease)
                    .await?
            }
            None => {
                pypi.get_latest_version(&pkg_config.name, pkg_config.allow_prerelease)
                    .await?
            }
        };

        let current = buildout.get_version(pkg_config.buildout_name());
        let has_update = current.map_or(true, |c| c != latest.version);

        updates.push(UpdateInfo {
            package: pkg_config.name.clone(),
            buildout_name: pkg_config.buildout_name().to_string(),
            current_version: current.map(|s| s.to_string()),
            latest_version: latest.version,
            has_update,
        });

        if let Some(pb) = progress.as_ref() {
            pb.inc(1);
        }
    }

    if let Some(pb) = progress {
        pb.finish_with_message("Package check complete");
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&updates).unwrap());
    } else {
        print_update_table(&updates);
    }

    Ok(())
}

async fn cmd_update(
    config_path: &str,
    packages_filter: Option<String>,
    auto_confirm: bool,
    dry_run: bool,
    commit: bool,
    push: bool,
    non_interactive: bool,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;

    let commit = commit || push;
    let git = GitOps::new();

    if commit {
        if !git.is_repo() {
            return Err(ReleaserError::GitError(
                "Not in a git repository".to_string(),
            ));
        }

        if !git.is_clean()? {
            if non_interactive {
                return Err(ReleaserError::GitError(
                    "Uncommitted changes detected. Clean your workspace or rerun without --non-interactive.".to_string(),
                ));
            }

            println!("{}", "Warning: You have uncommitted changes.".yellow());
            let proceed = Confirm::new()
                .with_prompt("Do you want to continue? (changes will be included in the commit)")
                .default(false)
                .interact()
                .map_err(|e| {
                    ReleaserError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;

            if !proceed {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    let updates = perform_update(
        &config,
        packages_filter,
        auto_confirm || non_interactive,
        dry_run,
        verbose,
    )
    .await?;

    if updates.is_empty() {
        return Ok(());
    }

    if dry_run {
        if commit {
            println!("{}", "Dry run: skipping commit/push actions.".yellow());
        }
        return Ok(());
    }

    if commit {
        let commit_message =
            generate_commit_message(&updates, config.git.effective_commit_template(), None);
        if verbose {
            println!("Commit message: {}", commit_message);
        }

        git.add(&config.versions_file)?;
        println!("{} Staged {}", "✓".green(), config.versions_file);

        git.commit(&commit_message)?;
        println!("{} Committed changes", "✓".green());

        if push {
            git.push(false)?;
            println!("{} Pushed to remote", "✓".green());
        }
    }

    Ok(())
}

fn cmd_release(
    config_path: &str,
    tag: Option<String>,
    bump: Option<String>,
    message: Option<&str>,
    no_push: bool,
    no_github: bool,
    draft: bool,
    no_metadata: bool,
    non_interactive: bool,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;
    let git = GitOps::new();

    // Verify we're in a git repo
    if !git.is_repo() {
        return Err(ReleaserError::GitError(
            "Not in a git repository".to_string(),
        ));
    }

    // Resolve version
    let version_str = resolve_version(&config, &git, tag, bump, verbose)?;

    // Check for uncommitted changes
    if !git.is_clean()? {
        if non_interactive {
            return Err(ReleaserError::GitError(
                "Uncommitted changes detected. Clean your workspace or rerun without --non-interactive.".to_string(),
            ));
        }

        println!("{}", "Warning: You have uncommitted changes.".yellow());

        let proceed = Confirm::new()
            .with_prompt("Do you want to continue?")
            .default(false)
            .interact()
            .map_err(|e| {
                ReleaserError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        if !proceed {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Update metadata files
    let updated_metadata = if !no_metadata && !config.metadata_files.is_empty() {
        let date = current_date();
        println!("{}", "Updating metadata files...".cyan());
        let files = MetadataUpdater::update_all(&config.metadata_files, &version_str, &date)?;
        for file in &files {
            println!("{} Updated {}", "✓".green(), file);
        }
        files
    } else {
        Vec::new()
    };

    // Stage metadata files
    for file in &updated_metadata {
        git.add(file)?;
    }

    // Commit if we have changes
    if !updated_metadata.is_empty() {
        let commit_msg = format!("Bump version to {}", version_str);
        git.commit(&commit_msg)?;
        println!("{} Committed metadata changes", "✓".green());
    }

    perform_release(
        &config,
        &version_str,
        message,
        no_push,
        no_github,
        draft,
        verbose,
    )
}

fn cmd_version(
    config_path: &str,
    bump: Option<String>,
    list_levels: bool,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;
    let git = GitOps::new();
    let version_manager = VersionManager::new(&config.version);

    if verbose {
        println!("Using config: {}", config_path);
    }

    if list_levels {
        println!("{}", "Available version bump levels:".cyan().bold());
        let mut levels: Vec<_> = version_manager.available_levels();
        levels.sort_by_key(|(name, _)| *name);

        for (name, bump_type) in levels {
            let desc = match bump_type {
                config::VersionBumpType::Major => "X.0.0 (breaking changes)",
                config::VersionBumpType::Minor => "0.X.0 (new features)",
                config::VersionBumpType::Patch => "0.0.X (bug fixes)",
            };
            println!("  {:<12} → {}", name.yellow(), desc);
        }
        return Ok(());
    }

    // Get current version from git tags
    let current = git.get_latest_version(&config.github.tag_prefix)?;

    match current {
        Some(version) => {
            println!(
                "Current version (from git tags): {}",
                version.to_string().green()
            );

            if let Some(level) = bump {
                let bump_type = version_manager.get_bump_type(&level)?;
                let next = version.bump(bump_type);
                println!("Next version ({}): {}", level, next.to_string().yellow());
            }
        }
        None => {
            println!("{}", "No version tags found.".yellow());
            println!("First release will be: {}", "0.1.0".green());

            if let Some(level) = bump {
                let initial = Version::new(0, 0, 0);
                let bump_type = version_manager.get_bump_type(&level)?;
                let next = initial.bump(bump_type);
                println!("First version ({}): {}", level, next.to_string().yellow());
            }
        }
    }

    Ok(())
}

async fn cmd_update_release(
    config_path: &str,
    tag: Option<String>,
    bump: Option<String>,
    packages_filter: Option<String>,
    auto_confirm: bool,
    custom_message: Option<String>,
    no_push: bool,
    no_github: bool,
    draft: bool,
    dry_run: bool,
    changelog_flag: bool,
    no_changelog_flag: bool,
    changelog_format_override: Option<CliChangelogFormat>,
    changelog_file_override: Option<String>,
    no_metadata: bool,
    non_interactive: bool,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;
    let git = GitOps::new();

    // Verify we're in a git repo
    if !git.is_repo() {
        return Err(ReleaserError::GitError(
            "Not in a git repository".to_string(),
        ));
    }

    // Resolve version
    let version_str = resolve_version(&config, &git, tag, bump, verbose)?;

    let auto_confirm = auto_confirm || non_interactive;

    // Determine changelog settings
    let collect_changelog = if no_changelog_flag {
        false
    } else if changelog_flag {
        true
    } else {
        config.changelog.enabled
    };

    let changelog_format = changelog_format_override
        .map(|f| f.into())
        .unwrap_or_else(|| config.changelog.format_enum());

    let changelog_file = changelog_file_override.or_else(|| config.changelog.output_file.clone());

    // Check for uncommitted changes
    if !git.is_clean()? {
        if non_interactive {
            return Err(ReleaserError::GitError(
                "Uncommitted changes detected. Clean your workspace or rerun without --non-interactive.".to_string(),
            ));
        }

        println!("{}", "Warning: You have uncommitted changes.".yellow());

        if !auto_confirm {
            let proceed = Confirm::new()
                .with_prompt("Do you want to continue? (changes will be included in the commit)")
                .default(false)
                .interact()
                .map_err(|e| {
                    ReleaserError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;

            if !proceed {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    println!("{}", "═".repeat(60).cyan());
    println!("{}", " STEP 1: Update Packages".cyan().bold());
    println!("{}", "═".repeat(60).cyan());

    // Perform updates
    let updates = perform_update(&config, packages_filter, auto_confirm, dry_run, verbose).await?;

    if updates.is_empty() {
        if !auto_confirm {
            let proceed = Confirm::new()
                .with_prompt("No package updates. Do you still want to create a release?")
                .default(false)
                .interact()
                .map_err(|e| {
                    ReleaserError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))
                })?;

            if !proceed {
                println!("Aborted.");
                return Ok(());
            }
        } else {
            println!("{}", "No updates available, skipping release.".yellow());
            return Ok(());
        }
    }

    // Collect changelogs
    let consolidated_changelog = if collect_changelog && !updates.is_empty() {
        println!("\n{}", "═".repeat(60).cyan());
        println!("{}", " STEP 2: Collecting Changelogs".cyan().bold());
        println!("{}", "═".repeat(60).cyan());

        let collector = ChangelogCollector::with_config(&config.changelog);
        let spinner = create_spinner("Fetching changelogs from packages...");

        let changelogs = collector
            .collect_changelogs(&updates, &config.packages)
            .await?;

        spinner.finish_with_message("Changelog collection complete");

        let found_count = changelogs.iter().filter(|c| !c.entries.is_empty()).count();
        println!(
            "{} Found changelog entries for {}/{} packages",
            "✓".green(),
            found_count,
            changelogs.len()
        );

        Some(ConsolidatedChangelog::with_templates(
            &version_str,
            &current_date(),
            changelogs,
            &config.changelog,
        ))
    } else {
        None
    };

    // Update metadata files
    let updated_metadata = if !no_metadata && !config.metadata_files.is_empty() && !dry_run {
        let step = if collect_changelog { 3 } else { 2 };
        println!("\n{}", "═".repeat(60).cyan());
        println!(
            "{}",
            format!(" STEP {}: Update Metadata Files", step)
                .cyan()
                .bold()
        );
        println!("{}", "═".repeat(60).cyan());

        let date = current_date();
        let files = MetadataUpdater::update_all(&config.metadata_files, &version_str, &date)?;
        for file in &files {
            println!("{} Updated {}", "✓".green(), file);
        }
        files
    } else {
        Vec::new()
    };

    if dry_run {
        println!("\n{}", "═".repeat(60).cyan());
        println!("{}", " DRY RUN: Release Preview".cyan().bold());
        println!("{}", "═".repeat(60).cyan());

        let commit_message = generate_commit_message(
            &updates,
            config.git.effective_commit_template(),
            custom_message.as_deref(),
        );
        let full_tag = format!("{}{}", config.github.tag_prefix, version_str);

        println!("\nWould perform the following actions:");
        println!("  Version: {}", version_str.yellow());
        println!("  1. Stage file: {}", config.versions_file);

        if !no_metadata {
            for meta in &config.metadata_files {
                println!("  2. Update metadata: {}", meta.path);
            }
        }

        println!("  3. Commit with message:");
        println!("     {}", commit_message.dimmed());
        println!("  4. Create tag: {}", full_tag.yellow());

        if !no_push {
            println!("  5. Push to remote (with tags)");
        }

        if !no_github && config.github.create_release {
            println!(
                "  6. Create GitHub release{}",
                if draft { " (draft)" } else { "" }
            );
        }

        if let Some(ref changelog) = consolidated_changelog {
            println!("\n{}", "Generated Changelog:".cyan().bold());
            println!("{}", "-".repeat(40));
            let output = changelog.render(changelog_format);
            for (i, line) in output.lines().enumerate() {
                if i >= 50 {
                    println!("... (truncated)");
                    break;
                }
                println!("{}", line);
            }
        }

        println!("\n{}", "Dry run complete - no changes made.".yellow());
        return Ok(());
    }

    // Save changelog
    if let Some(ref changelog) = consolidated_changelog {
        if let Some(ref file_path) = changelog_file {
            changelog.save_to_file(file_path, changelog_format)?;
            println!("{} Saved changelog to: {}", "✓".green(), file_path);
        }
    }

    let step_num = if collect_changelog { 4 } else { 3 };
    println!("\n{}", "═".repeat(60).cyan());
    println!(
        "{}",
        format!(" STEP {}: Commit Changes", step_num).cyan().bold()
    );
    println!("{}", "═".repeat(60).cyan());

    // Generate commit message
    let commit_message = generate_commit_message(
        &updates,
        config.git.effective_commit_template(),
        custom_message.as_deref(),
    );

    if verbose {
        println!("Commit message: {}", commit_message);
    }

    // Stage files
    git.add(&config.versions_file)?;
    println!("{} Staged {}", "✓".green(), config.versions_file);

    // Stage changelog
    if config.changelog.include_in_commit {
        if let Some(ref file_path) = changelog_file {
            git.add(file_path)?;
            println!("{} Staged {}", "✓".green(), file_path);
        }
    }

    // Stage metadata files
    for file in &updated_metadata {
        if config
            .metadata_files
            .iter()
            .any(|m| &m.path == file && m.include_in_commit)
        {
            git.add(file)?;
            println!("{} Staged {}", "✓".green(), file);
        }
    }

    // Commit
    git.commit(&commit_message)?;
    println!("{} Committed changes", "✓".green());

    let step_num = step_num + 1;
    println!("\n{}", "═".repeat(60).cyan());
    println!(
        "{}",
        format!(" STEP {}: Create Release", step_num).cyan().bold()
    );
    println!("{}", "═".repeat(60).cyan());

    // Create release message
    let release_notes = if config.changelog.use_as_release_notes {
        if let Some(ref changelog) = consolidated_changelog {
            changelog.render(changelog_format)
        } else {
            generate_release_notes(&updates, &version_str)
        }
    } else {
        generate_release_notes(&updates, &version_str)
    };

    let release_message = custom_message.as_deref().unwrap_or(&release_notes);

    perform_release(
        &config,
        &version_str,
        Some(release_message),
        no_push,
        no_github,
        draft,
        verbose,
    )?;

    println!("\n{}", "═".repeat(60).green());
    println!("{}", " Release Complete!".green().bold());
    println!("{}", "═".repeat(60).green());

    let full_tag = format!("{}{}", config.github.tag_prefix, version_str);
    println!("\nSummary:");
    println!("  • Version: {}", version_str.yellow());
    println!("  • Updated {} package(s)", updates.len());
    if consolidated_changelog.is_some() {
        println!("  • Collected changelogs");
    }
    if let Some(ref file_path) = changelog_file {
        println!("  • Saved changelog to: {}", file_path);
    }
    if !updated_metadata.is_empty() {
        println!("  • Updated {} metadata file(s)", updated_metadata.len());
    }
    println!("  • Created tag: {}", full_tag.yellow());
    if !no_push {
        println!("  • Pushed to remote");
    }
    if !no_github && config.github.create_release {
        println!(
            "  • Created GitHub release{}",
            if draft { " (draft)" } else { "" }
        );
    }

    Ok(())
}
async fn cmd_changelog(
    config_path: &str,
    packages_filter: Option<String>,
    format_override: Option<CliChangelogFormat>,
    output_file_override: Option<String>,
    force_stdout: bool,
    release_version: Option<String>,
    rebuild: bool,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(config_path)?;

    let format = format_override
        .map(|f| f.into())
        .unwrap_or_else(|| config.changelog.format_enum());

    let output_file = if force_stdout {
        None
    } else {
        output_file_override.or_else(|| config.changelog.output_file.clone())
    };

    let packages_to_check = filter_packages(&config.packages, packages_filter.as_deref());

    if rebuild {
        return rebuild_changelog_from_tags(
            &config,
            &packages_to_check,
            format,
            output_file,
            verbose,
        )
        .await;
    }

    let pypi = PyPiClient::new()?;
    let buildout = BuildoutVersions::load(&config.versions_file)?;

    println!("{}", "Checking for updates...".cyan());

    let mut updates = Vec::new();

    for pkg_config in &packages_to_check {
        if verbose {
            println!("  Checking {}...", pkg_config.name);
        }

        let latest = match &pkg_config.version_constraint {
            Some(constraint) => {
                pypi.get_matching_version(&pkg_config.name, constraint, pkg_config.allow_prerelease)
                    .await?
            }
            None => {
                pypi.get_latest_version(&pkg_config.name, pkg_config.allow_prerelease)
                    .await?
            }
        };

        let current = buildout.get_version(pkg_config.buildout_name());

        if let Some(current_version) = current {
            if current_version != latest.version {
                updates.push(VersionUpdate {
                    package_name: pkg_config.buildout_name().to_string(),
                    old_version: current_version.to_string(),
                    new_version: latest.version,
                });
            }
        }
    }

    if updates.is_empty() {
        println!("{}", "All packages are up to date!".green());
        return Ok(());
    }

    println!(
        "\n{} Found {} package(s) with updates",
        "✓".green(),
        updates.len()
    );

    println!("{}", "\nFetching changelogs...".cyan());

    let collector = ChangelogCollector::with_config(&config.changelog);
    let changelogs = collector
        .collect_changelogs(&updates, &config.packages)
        .await?;

    let found_count = changelogs.iter().filter(|c| !c.entries.is_empty()).count();
    println!(
        "{} Found changelog entries for {}/{} packages",
        "✓".green(),
        found_count,
        changelogs.len()
    );

    let version = release_version.unwrap_or_else(|| "UNRELEASED".to_string());
    let consolidated = ConsolidatedChangelog::with_templates(
        &version,
        &current_date(),
        changelogs,
        &config.changelog,
    );

    match output_file {
        Some(path) => {
            consolidated.save_to_file(&path, format)?;
            println!("\n{} Changelog saved to: {}", "✓".green(), path);
        }
        None => {
            println!("\n{}", "═".repeat(60));
            println!("{}", consolidated.render(format));
        }
    }

    Ok(())
}

fn cmd_add(
    config_path: &str,
    package: &str,
    constraint: Option<String>,
    buildout_name: Option<String>,
    changelog_url: Option<String>,
) -> Result<()> {
    let mut config = Config::load(config_path)?;

    if config.packages.iter().any(|p| p.name == package) {
        return Err(ReleaserError::ConfigError(format!(
            "Package '{}' is already configured",
            package
        )));
    }

    config.packages.push(PackageConfig {
        name: package.to_string(),
        version_constraint: constraint,
        buildout_name,
        allow_prerelease: false,
        changelog_url,
    });

    config.save(config_path)?;
    println!("{} Added package: {}", "✓".green(), package);

    Ok(())
}

fn cmd_remove(config_path: &str, package: &str) -> Result<()> {
    let mut config = Config::load(config_path)?;

    let initial_len = config.packages.len();
    config.packages.retain(|p| p.name != package);

    if config.packages.len() == initial_len {
        return Err(ReleaserError::ConfigError(format!(
            "Package '{}' not found in configuration",
            package
        )));
    }

    config.save(config_path)?;
    println!("{} Removed package: {}", "✓".green(), package);

    Ok(())
}

async fn cmd_list(config_path: &str, detailed: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    let buildout = BuildoutVersions::load(&config.versions_file).ok();

    if config.packages.is_empty() {
        println!("No packages configured.");
        return Ok(());
    }

    println!("{}", "Tracked packages:".cyan().bold());

    for pkg in &config.packages {
        let current_version = buildout
            .as_ref()
            .and_then(|b| b.get_version(pkg.buildout_name()))
            .unwrap_or("not set");

        if detailed {
            println!("\n  {}", pkg.name.yellow().bold());
            println!("    Current version: {}", current_version);
            if let Some(ref constraint) = pkg.version_constraint {
                println!("    Constraint: {}", constraint);
            }
            if let Some(ref bn) = pkg.buildout_name {
                println!("    Buildout name: {}", bn);
            }
            if pkg.allow_prerelease {
                println!("    Pre-releases: allowed");
            }
            if let Some(ref url) = pkg.changelog_url {
                println!("    Changelog URL: {}", url);
            }
        } else {
            let constraint_str = pkg
                .version_constraint
                .as_ref()
                .map(|c| format!(" ({})", c))
                .unwrap_or_default();

            println!(
                "  {} = {}{}",
                pkg.buildout_name(),
                current_version,
                constraint_str.dimmed()
            );
        }
    }

    Ok(())
}

async fn cmd_info(package: &str, show_versions: bool) -> Result<()> {
    let pypi = PyPiClient::new()?;
    let info = pypi.get_package_info(package).await?;

    println!("{}", info.info.name.yellow().bold());
    println!("  Latest version: {}", info.info.version.green());

    if let Some(ref summary) = info.info.summary {
        println!("  Summary: {}", summary);
    }

    if let Some(ref urls) = info.info.project_urls {
        if let Some(homepage) = urls.get("Homepage").or(info.info.home_page.as_ref()) {
            println!("  Homepage: {}", homepage);
        }
    }

    if show_versions {
        println!("\n  {}", "Available versions:".cyan());

        let mut versions: Vec<_> = info.releases.keys().collect();
        versions.sort_by(
            |a, b| match (semver::Version::parse(a), semver::Version::parse(b)) {
                (Ok(va), Ok(vb)) => vb.cmp(&va),
                _ => b.cmp(a),
            },
        );

        for version in versions.iter().take(20) {
            let yanked = info
                .releases
                .get(*version)
                .map(|r| r.iter().all(|ri| ri.yanked))
                .unwrap_or(false);

            if yanked {
                println!("    {} {}", version, "(yanked)".red());
            } else {
                println!("    {}", version);
            }
        }

        if versions.len() > 20 {
            println!("    ... and {} more", versions.len() - 20);
        }
    }

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Resolve version from tag or bump
fn resolve_version(
    config: &Config,
    git: &GitOps,
    tag: Option<String>,
    bump: Option<String>,
    verbose: bool,
) -> Result<String> {
    // Explicit tag takes precedence
    if let Some(tag) = tag {
        return Ok(tag);
    }

    // Bump from latest git tag
    if let Some(level) = bump {
        let version_manager = VersionManager::new(&config.version);
        let bump_type = version_manager.get_bump_type(&level)?;

        let current = git.get_latest_version(&config.github.tag_prefix)?;

        let next = match current {
            Some(version) => {
                if verbose {
                    println!(
                        "Current version (from tag): {} → bumping {}",
                        version, level
                    );
                }
                version.bump(bump_type)
            }
            None => {
                if verbose {
                    println!("No existing version tags found, starting from 0.0.0");
                }
                // Start from 0.0.0 and bump
                Version::new(0, 0, 0).bump(bump_type)
            }
        };

        if verbose {
            println!("Next version: {}", next);
        }

        return Ok(next.to_string());
    }

    Err(ReleaserError::ConfigError(
        "Either --tag or --bump must be specified".to_string(),
    ))
}

fn create_progress_bar(len: usize, message: &str) -> Option<ProgressBar> {
    if len == 0 {
        return None;
    }

    let pb = ProgressBar::new(len as u64);
    pb.set_style(
        ProgressStyle::with_template(
            " {msg}\n {spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len}",
        )
        .expect("progress template should be valid")
        .progress_chars("=>-"),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(120));

    Some(pb)
}

fn create_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template(" {spinner:.cyan} {msg}")
            .expect("spinner template should be valid")
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(120));
    pb
}

async fn perform_update(
    config: &Config,
    packages_filter: Option<String>,
    auto_confirm: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<Vec<VersionUpdate>> {
    let pypi = PyPiClient::new()?;
    let mut buildout = BuildoutVersions::load(&config.versions_file)?;

    let packages_to_check = filter_packages(&config.packages, packages_filter.as_deref());

    let mut available_updates = Vec::new();

    println!("{}", "Checking for updates...".cyan());

    let progress = create_progress_bar(packages_to_check.len(), "Checking packages");

    for pkg_config in &packages_to_check {
        if let Some(pb) = progress.as_ref() {
            pb.set_message(format!("Checking {}...", pkg_config.name));
            if verbose {
                pb.println(format!("Checking {}...", pkg_config.name));
            }
        } else if verbose {
            println!("  Checking {}...", pkg_config.name);
        }

        let latest = match &pkg_config.version_constraint {
            Some(constraint) => {
                pypi.get_matching_version(&pkg_config.name, constraint, pkg_config.allow_prerelease)
                    .await?
            }
            None => {
                pypi.get_latest_version(&pkg_config.name, pkg_config.allow_prerelease)
                    .await?
            }
        };

        let current = buildout.get_version(pkg_config.buildout_name());

        if let Some(current_version) = current {
            if current_version != latest.version {
                available_updates.push((
                    pkg_config.buildout_name().to_string(),
                    current_version.to_string(),
                    latest.version,
                ));
            }
        }

        if let Some(pb) = progress.as_ref() {
            pb.inc(1);
        }
    }

    if let Some(pb) = progress {
        pb.finish_with_message("Update check complete");
    }

    if available_updates.is_empty() {
        println!("{}", "All packages are up to date!".green());
        return Ok(Vec::new());
    }

    println!("\n{}", "Available updates:".yellow().bold());
    for (name, current, latest) in &available_updates {
        println!("  {} {} → {}", name, current.dimmed(), latest.green());
    }

    let selected_updates = if auto_confirm {
        available_updates.clone()
    } else {
        let items: Vec<String> = available_updates
            .iter()
            .map(|(name, current, latest)| format!("{}: {} → {}", name, current, latest))
            .collect();

        let selections = MultiSelect::new()
            .with_prompt("Select packages to update")
            .items(&items)
            .defaults(&vec![true; items.len()])
            .interact()
            .map_err(|e| {
                ReleaserError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        selections
            .iter()
            .map(|&i| available_updates[i].clone())
            .collect()
    };

    if selected_updates.is_empty() {
        println!("No updates selected.");
        return Ok(Vec::new());
    }

    let mut applied_updates = Vec::new();

    for (name, _current, latest) in &selected_updates {
        if let Some(update) = buildout.update_version(name, latest)? {
            applied_updates.push(update);
            if verbose {
                println!("  {} Updated {} to {}", "✓".green(), name, latest);
            }
        }
    }

    if dry_run {
        println!("\n{}", "Dry run - no files were modified.".yellow());
        println!("Would update:");
        for update in &applied_updates {
            println!(
                "  {} {} → {}",
                update.package_name, update.old_version, update.new_version
            );
        }
    } else {
        buildout.save()?;
        println!(
            "\n{} Updated {} package(s)",
            "✓".green(),
            applied_updates.len()
        );
    }

    Ok(applied_updates)
}

fn perform_release(
    config: &Config,
    tag: &str,
    message: Option<&str>,
    no_push: bool,
    no_github: bool,
    draft: bool,
    verbose: bool,
) -> Result<()> {
    let git = GitOps::new();

    if !git.is_repo() {
        return Err(ReleaserError::GitError(
            "Not in a git repository".to_string(),
        ));
    }

    let full_tag = format!("{}{}", config.github.tag_prefix, tag);
    let default_message = format!("Release {}", tag);
    let release_message = message.unwrap_or(&default_message);

    if verbose {
        println!("Creating tag: {}", full_tag);
    }

    git.tag(&full_tag, Some(release_message))?;
    println!("{} Created tag: {}", "✓".green(), full_tag);

    if !no_push {
        if verbose {
            println!("Pushing to remote...");
        }
        git.push(true)?;
        println!("{} Pushed to remote", "✓".green());
    }

    if !no_github && config.github.create_release {
        if !GitHubOps::is_available() {
            println!(
                "{} GitHub CLI (gh) not found, skipping GitHub release",
                "⚠".yellow()
            );
        } else if !GitHubOps::is_authenticated()? {
            println!(
                "{} Not authenticated to GitHub, skipping release",
                "⚠".yellow()
            );
            println!("  Run 'gh auth login' to authenticate");
        } else {
            if verbose {
                println!("Creating GitHub release...");
            }

            GitHubOps::create_release(
                &full_tag,
                Some(&format!("Release {}", tag)),
                Some(release_message),
                draft,
                false,
            )?;

            println!("{} Created GitHub release", "✓".green());
        }
    }

    Ok(())
}

fn filter_packages(packages: &[PackageConfig], filter: Option<&str>) -> Vec<PackageConfig> {
    match filter {
        Some(f) => {
            let names: Vec<&str> = f.split(',').map(|s| s.trim()).collect();
            packages
                .iter()
                .filter(|p| names.contains(&p.name.as_str()))
                .cloned()
                .collect()
        }
        None => packages.to_vec(),
    }
}

fn generate_commit_message(
    updates: &[VersionUpdate],
    template: &str,
    custom: Option<&str>,
) -> String {
    if let Some(msg) = custom {
        return msg.to_string();
    }

    let packages_str = match updates.len() {
        0 => String::new(),
        1 => format!("{} = {}", updates[0].package_name, updates[0].new_version),
        _ => {
            let all_but_last: Vec<_> = updates[..updates.len() - 1]
                .iter()
                .map(|u| format!("{} = {}", u.package_name, u.new_version))
                .collect();
            let last = updates.last().unwrap();
            format!(
                "{} and {} = {}",
                all_but_last.join(", "),
                last.package_name,
                last.new_version
            )
        }
    };

    let effective_template = if template.trim().is_empty() {
        "Use {packages}"
    } else {
        template
    };

    let date = current_date();

    effective_template
        .replace("{packages}", &packages_str)
        .replace("{date}", &date)
}

fn generate_release_notes(updates: &[VersionUpdate], tag: &str) -> String {
    let mut notes = format!("## Release {}\n\n", tag);

    if !updates.is_empty() {
        notes.push_str("### Package Updates\n\n");
        for update in updates {
            notes.push_str(&format!(
                "- **{}**: {} → {}\n",
                update.package_name, update.old_version, update.new_version
            ));
        }
    }

    notes
}

fn current_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let days_since_epoch = secs / 86400;

    let mut year = 1970i32;
    let mut remaining_days = days_since_epoch as i32;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let days_in_months: [i32; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 0;
    for (i, &days) in days_in_months.iter().enumerate() {
        if remaining_days < days {
            month = i + 1;
            break;
        }
        remaining_days -= days;
    }

    let day = remaining_days + 1;

    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ============================================================================
// Data Structures
// ============================================================================

#[derive(serde::Serialize)]
struct UpdateInfo {
    package: String,
    buildout_name: String,
    current_version: Option<String>,
    latest_version: String,
    has_update: bool,
}

fn print_update_table(updates: &[UpdateInfo]) {
    let has_updates = updates.iter().any(|u| u.has_update);

    if !has_updates {
        println!("{}", "All packages are up to date!".green());
        return;
    }

    println!(
        "\n{:<30} {:<15} {:<15} {}",
        "Package", "Current", "Latest", "Status"
    );
    println!("{}", "-".repeat(70));

    for update in updates {
        let current = update.current_version.as_deref().unwrap_or("not set");
        let status = if update.has_update {
            "UPDATE AVAILABLE".yellow()
        } else {
            "up to date".green()
        };

        println!(
            "{:<30} {:<15} {:<15} {}",
            update.buildout_name, current, update.latest_version, status
        );
    }
}
