# Windows development setup. Installs Node 20 when it is missing, then runs
# scripts/setup.mjs. Safe to run again. No prompts.
#
#   powershell -File scripts/setup.ps1

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $scriptDir) { $scriptDir = Join-Path (Get-Location) 'scripts' }
$root = Split-Path -Parent $scriptDir
Set-Location $root

function Have([string]$Name) {
  return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

$windows = $env:OS -eq 'Windows_NT'

if (-not (Have 'node')) {
  if (-not $windows) {
    Write-Error 'node was not found. On macOS and Linux run: bash scripts/setup.sh'
    exit 1
  }
  $arch = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
  Write-Host "setup.ps1: installing Node 20 for win-$arch"
  $sums = (Invoke-RestMethod 'https://nodejs.org/dist/latest-v20.x/SHASUMS256.txt') -split "`n"
  $name = ($sums | Where-Object { $_ -match "node-v[0-9.]+-win-$arch\.zip$" } | Select-Object -First 1)
  if (-not $name) { Write-Error "Node 20 zip for win-$arch was not listed"; exit 1 }
  $parts = $name.Trim() -split '\s+', 2
  $hash = $parts[0].ToLower()
  $file = $parts[1].Trim()
  $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("locus-node-" + [guid]::NewGuid().ToString('n'))
  New-Item -ItemType Directory -Path $tmp | Out-Null
  $zip = Join-Path $tmp $file
  Invoke-WebRequest -Uri "https://nodejs.org/dist/latest-v20.x/$file" -OutFile $zip
  $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
  if ($actual -ne $hash) { Write-Error 'Node zip checksum mismatch'; exit 1 }
  Expand-Archive -LiteralPath $zip -DestinationPath $tmp -Force
  $extracted = Join-Path $tmp ($file -replace '\.zip$', '')
  $dest = Join-Path $env:LOCALAPPDATA 'locus-setup\node'
  if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
  New-Item -ItemType Directory -Path (Split-Path $dest) -Force | Out-Null
  Move-Item $extracted $dest
  $env:Path = (Join-Path $dest 'bin') + [IO.Path]::PathSeparator + $env:Path
  Remove-Item -Recurse -Force $tmp
}

& node (Join-Path $scriptDir 'setup.mjs')
exit $LASTEXITCODE
