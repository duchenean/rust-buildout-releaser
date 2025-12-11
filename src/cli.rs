use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "buildout-releaser")]
#[command(author, version, about = "A zc.buildout package releaser tool", long_about = None)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "releaser.toml")]
    pub config: String,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum CliChangelogFormat {
    Markdown,
    Rst,
    Text,
}

impl From<CliChangelogFormat> for crate::config::ChangelogFormat {
    fn from(f: CliChangelogFormat) -> Self {
        match f {
            CliChangelogFormat::Markdown => crate::config::ChangelogFormat::Markdown,
            CliChangelogFormat::Rst => crate::config::ChangelogFormat::Rst,
            CliChangelogFormat::Text => crate::config::ChangelogFormat::Text,
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate shell completion scripts
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Initialize a new configuration file
    Init {
        /// Force overwrite existing config
        #[arg(short, long)]
        force: bool,
    },

    /// Check for available updates
    Check {
        /// Only check specific packages (comma-separated)
        #[arg(short, long)]
        packages: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Update package versions in buildout file
    Update {
        /// Only update specific packages (comma-separated)
        #[arg(short, long)]
        packages: Option<String>,

        /// Don't prompt for confirmation
        #[arg(short = 'y', long)]
        yes: bool,

        /// Dry run - don't actually modify files
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Create a release (commit, tag, and optionally push)
    Release {
        /// Version tag for the release (or use --bump)
        #[arg(short, long, required_unless_present = "bump")]
        tag: Option<String>,

        /// Bump version level (e.g., major, minor, patch, fix)
        #[arg(short, long, required_unless_present = "tag")]
        bump: Option<String>,

        /// Release notes/message
        #[arg(short, long)]
        message: Option<String>,

        /// Don't push to remote
        #[arg(long)]
        no_push: bool,

        /// Don't create GitHub release
        #[arg(long)]
        no_github: bool,

        /// Create as draft release
        #[arg(long)]
        draft: bool,

        /// Don't update metadata files (publiccode.yml, etc.)
        #[arg(long)]
        no_metadata: bool,
    },

    /// Update packages and create a release in one step
    UpdateRelease {
        /// Version tag for the release (or use --bump)
        #[arg(short, long, required_unless_present = "bump")]
        tag: Option<String>,

        /// Bump version level (e.g., major, minor, patch, fix)
        #[arg(short, long, required_unless_present = "tag")]
        bump: Option<String>,

        /// Only update specific packages (comma-separated)
        #[arg(short, long)]
        packages: Option<String>,

        /// Don't prompt for confirmation
        #[arg(short = 'y', long)]
        yes: bool,

        /// Custom release message
        #[arg(short, long)]
        message: Option<String>,

        /// Don't push to remote
        #[arg(long)]
        no_push: bool,

        /// Don't create GitHub release
        #[arg(long)]
        no_github: bool,

        /// Create as draft release
        #[arg(long)]
        draft: bool,

        /// Dry run - show what would happen
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Collect changelogs (overrides config)
        #[arg(long)]
        changelog: bool,

        /// Disable changelog collection (overrides config)
        #[arg(long, conflicts_with = "changelog")]
        no_changelog: bool,

        /// Changelog output format (overrides config)
        #[arg(long, value_enum)]
        changelog_format: Option<CliChangelogFormat>,

        /// Save changelog to file (overrides config)
        #[arg(long)]
        changelog_file: Option<String>,

        /// Don't update metadata files (publiccode.yml, etc.)
        #[arg(long)]
        no_metadata: bool,
    },

    /// Collect changelogs for package updates
    Changelog {
        /// Only check specific packages (comma-separated)
        #[arg(short, long)]
        packages: Option<String>,

        /// Output format (overrides config)
        #[arg(short, long, value_enum)]
        format: Option<CliChangelogFormat>,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<String>,

        /// Release version for the changelog header
        #[arg(long)]
        release_version: Option<String>,
    },

    /// Show or bump version
    Version {
        /// Bump level to show next version (e.g., major, minor, patch)
        #[arg(short, long)]
        bump: Option<String>,

        /// List available bump levels
        #[arg(short, long)]
        list_levels: bool,
    },

    /// Add a package to track
    Add {
        /// Package name on PyPI
        package: String,

        /// Version constraint (e.g., ">=2.0,<3.0")
        #[arg(short, long)]
        constraint: Option<String>,

        /// Custom name in buildout file
        #[arg(long)]
        buildout_name: Option<String>,

        /// Custom changelog URL
        #[arg(long)]
        changelog_url: Option<String>,
    },

    /// Remove a package from tracking
    Remove {
        /// Package name
        package: String,
    },

    /// List tracked packages
    List {
        /// Show detailed info
        #[arg(short, long)]
        detailed: bool,
    },

    /// Show package info from PyPI
    Info {
        /// Package name
        package: String,

        /// Show all available versions
        #[arg(long)]
        versions: bool,
    },
}
