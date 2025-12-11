# bldr

A CLI tool for releasing and managing versions of zc.buildout packages. The commands are defined with [`clap`](https://docs.rs/clap), and the tool aims to streamline checking for updates, updating buildout versions, and creating releases.

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
