# rust-buildout-releaser

A CLI tool for releasing and managing versions of zc.buildout packages. The commands are defined with [`clap`](https://docs.rs/clap), and the tool aims to streamline checking for updates, updating buildout versions, and creating releases.

## Shell completions

You can generate shell completion scripts so your shell can auto-complete subcommands and flags:

```bash
# Bash
buildout-releaser completions bash > ~/.local/share/bash-completion/completions/buildout-releaser

# Zsh
buildout-releaser completions zsh > "${fpath[1]}/_buildout-releaser"
autoload -U compinit && compinit

# Fish
buildout-releaser completions fish > ~/.config/fish/completions/buildout-releaser.fish

# PowerShell
buildout-releaser completions powershell | Out-String | Set-Content "$PROFILE.CurrentUserAllHosts"
```

Re-run the relevant command whenever the CLI changes to keep completion scripts up to date.
