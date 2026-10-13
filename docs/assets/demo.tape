Output docs/assets/locus-demo.gif
Set Shell bash
Set FontSize 16
Set Width 1100
Set Height 640
Set Theme "Catppuccin Mocha"
Hide
Type 'export LOCUS_HOME=$(mktemp -d) PS1="$ " && clear' Enter
Show
Type "locus quickstart" Sleep 400ms Enter
Sleep 3s
Type 'eval "$(locus hook bash)"' Sleep 300ms Enter
Sleep 1s
Type "locus whoami" Sleep 300ms Enter
Sleep 3s
