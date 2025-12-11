# bldr

**bldr** is a fast, chatty CLI sidekick for releasing and managing versions of your `zc.buildout` packages. It keeps your versions file tidy, curates changelogs, and can even ship tags and GitHub releases for you—all from one command.

## Why you'll love it

- 🚀 **One-command release flow** – jump from dependency updates to tagged releases (and optional GitHub releases) in a single `update-release` run.
- 🧭 **Smart tracking** – follow the packages you care about with optional constraints, buildout aliases, and prerelease toggles.
- 🧾 **Changelogs on tap** – collect markdown/RST/text notes automatically and feed them straight into commits or GitHub release notes.
- 🤝 **Git-aware** – commit updates, push, and tag with your preferred templates and branch targeting.
- 🛠️ **Config-first** – a simple `bldr.toml` drives everything so teams share the same rules.

## Install

### Prebuilt binaries (Linux)

Use the install helper to grab the latest release for your CPU (x86_64 or aarch64) and drop it on your PATH:

```bash
curl -sSfL https://raw.githubusercontent.com/maestropandy/rust-buildout-releaser/main/scripts/install.sh | sudo sh
# or
wget -qO- https://raw.githubusercontent.com/maestropandy/rust-buildout-releaser/main/scripts/install.sh | sudo sh
```

You can override the install directory (`BLDR_INSTALL_DIR`), target repo (`BLDR_REPO`), or pin a release (`BLDR_VERSION=v0.1.0`).

### From source

```bash
cargo install --path .
```

## Quick start

```bash
# 1) Create a default bldr.toml
bldr init

# 2) Track the packages you care about
bldr add pyramid --constraint ">=2.0,<3.0"

# 3) See what changed
bldr check

# 4) Update your versions file and commit the result
bldr update --yes

# 5) Ship it! Tag, (optionally) create a GitHub release, and push
bldr release --bump minor
```

The default configuration points to a buildout `versions.cfg`, but you can pass `--config` to any command to use another file.

## Commands at a glance

- Global flags:
  - `--config <path>` – choose a specific `bldr.toml`.
  - `--verbose` – print extra context while commands run.
  - `--non-interactive` – skip prompts for CI or other non-TTY environments.

- `init` – scaffold a fresh `bldr.toml` (use `--force` to overwrite).
- `add` / `remove` – manage tracked packages with optional constraints, buildout aliases, and changelog URLs.
- `list` – see everything you track (add `--detailed` for extra metadata).
- `check` – compare tracked packages against PyPI (add `--packages` or `--json`).
- `update` – write the newest versions into your buildout file; use `--yes` to skip prompts or `--dry-run` to preview.
- `release` – tag and commit a release with optional bumping (`--bump`), custom messages, and push/GitHub toggles.
- `update-release` – combine update + release in one shot; supports changelog collection (`--changelog` / `--no-changelog`), formats, draft releases, dry runs, and metadata updates.
- `changelog` – collect package changelogs in markdown/RST/text and write to stdout or a file (add `--stdout` to ignore configured files).
- `version` – display the current or bumped version; `--list-levels` shows available bump keywords.
- `info` – fetch PyPI metadata for a package; add `--versions` to list all releases.
- `completions` – generate shell completion scripts (see below).

## Configuration highlights (`bldr.toml`)

- **versions_file** – the buildout versions file to rewrite (e.g., `versions.cfg`).
- **packages** – objects with `name`, optional `version_constraint`, `buildout_name`, `allow_prerelease`, and `changelog_url`.
- **git** – target `branch`, `auto_push`, and a customizable `commit_template`.
- **github** – `repository` slug, `create_release` toggle, and optional `tag_prefix` (like `v`).
- **changelog** – enable collection by default, pick `format` (markdown/rst/text), choose an `output_file`, and control whether notes join the commit or GitHub release.
- **metadata_files** – extra files to refresh during releases (e.g., `publiccode.yml`).

Because the config is TOML, it is easy to review and share across your team’s repos.

## Shell completions

You can generate shell completion scripts so your shell can auto-complete subcommands and flags:

```bash
# Bash
bldr completions bash > ~/.local/share/bash-completion/completions/bldr

# Zsh
bldr completions zsh > "${fpath[1]}/_bldr"
autoload -U compinit && compinit

# Fish
bldr completions fish > ~/.config/fish/completions/bldr.fish

# PowerShell
bldr completions powershell | Out-String | Set-Content "$PROFILE.CurrentUserAllHosts"
```

Re-run the relevant command whenever the CLI changes to keep completion scripts up to date.

## Tips for smooth releases

- Run `bldr check` before `update` to see proposed changes.
- Use `--dry-run` when you want a preview without touching files.
- Pair `--no-github` or `--no-push` with `release`/`update-release` when testing locally.
- Customize changelog templates to match your team’s release notes style.

Now go ship something great—bldr’s got your back.
