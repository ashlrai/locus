# Shell completions

`locus completion <shell>` prints a completion script for `bash`, `zsh`,
`fish`, `elvish` or `powershell` to stdout. It reads no Locus config, so it
works before `locus init`.

Homebrew installs the bash, zsh and fish scripts for you. For npm, cargo or
source installs, install the script once for your shell and open a new shell.

## zsh

```zsh
mkdir -p ~/.zfunc
locus completion zsh > ~/.zfunc/_locus
```

Then in `~/.zshrc`, before `compinit` runs (oh-my-zsh runs it for you):

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

If completions don't appear, clear the cache: `rm -f ~/.zcompdump*`.

## bash

Needs the `bash-completion` package (Homebrew: `bash-completion@2`).

```bash
mkdir -p ~/.local/share/bash-completion/completions
locus completion bash > ~/.local/share/bash-completion/completions/locus
```

Without `bash-completion`, add `source <(locus completion bash)` to `~/.bashrc`.

## fish

```fish
locus completion fish > ~/.config/fish/completions/locus.fish
```

## PowerShell

Add this line to your profile (`notepad $PROFILE`):

```powershell
locus completion powershell | Out-String | Invoke-Expression
```

## elvish

Add this line to `~/.config/elvish/rc.elv`:

```elvish
eval (locus completion elvish | slurp)
```

## After upgrading

The script lists the commands and flags of the binary that generated it, so
regenerate it after upgrading Locus (Homebrew does this on upgrade).
