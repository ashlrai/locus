# One-command Locus install.
# macOS and Linux release archives are installed by scripts/install.sh.
# No Windows release archive is published, so this script installs the npm
# packages. On npm 11 those packages need an allow-scripts entry or their
# postinstall step does not run.
#
#   irm https://raw.githubusercontent.com/ashlrai/locus/main/scripts/install.ps1 | iex
#   powershell -File scripts/install.ps1

$ErrorActionPreference = 'Stop'
$windows = $env:OS -eq 'Windows_NT'

if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
  Write-Error 'npm was not found. Install Node.js from https://nodejs.org and run this script again.'
  exit 1
}

$version = (npm --version).Trim()
$major = [int]($version.Split('.')[0])
if ($major -ge 11) {
  npm install -g --allow-scripts=@ashlrai/locus-mcp,locus-cli locus-cli @ashlrai/locus-mcp
} else {
  npm install -g locus-cli @ashlrai/locus-mcp
}
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$prefix = (npm prefix -g).Trim()
$bin = if ($windows) { $prefix } else { Join-Path $prefix 'bin' }
$env:Path = $bin + [IO.Path]::PathSeparator + $env:Path

& locus --version
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& locus-mcp --version
exit $LASTEXITCODE
