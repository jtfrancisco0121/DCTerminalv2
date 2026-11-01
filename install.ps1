# Build DCTerminalv2 and install it for the current user on Windows 10/11.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1
#
# The installer is unsigned: Windows SmartScreen may show "Windows protected your PC". Choose
# "More info", then "Run anyway". Needs Git for Windows, Node.js and npm, Rust (stable, MSVC
# or GNU target) and Claude Code. Windows only; it refuses other systems and refuses while
# DCTerminalv2 is running (quit it first). Nothing else is touched.
$ErrorActionPreference = 'Stop'

function Say($text) { Write-Host "==> $text" }
function Die($text) { Write-Host "install.ps1: $text" -ForegroundColor Red; exit 1 }

# Windows only.
$onWindows = $false
if ($PSVersionTable.PSEdition -eq 'Desktop') { $onWindows = $true }
elseif (Get-Variable -Name IsWindows -ErrorAction SilentlyContinue) { $onWindows = [bool]$IsWindows }
if (-not $onWindows) { Die 'this installer is for Windows only (use install.sh on macOS).' }

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $Root

# Refuse while the app runs: replacing a running exe would fail half way.
$running = Get-Process -Name 'dcterminalv2', 'DCTerminalv2' -ErrorAction SilentlyContinue
if ($running) { Die 'DCTerminalv2 is running. Quit it (File, Quit), then run this again.' }

foreach ($tool in 'node', 'npm', 'cargo') {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Die "$tool is not on the PATH." }
}

Say 'npm ci'
npm ci
if ($LASTEXITCODE -ne 0) { Die 'npm ci failed.' }

Say 'building the app and the NSIS installer (npm run release:win)'
npm run release:win
if ($LASTEXITCODE -ne 0) { Die 'the build failed (see above; "dcv2-hook.exe is not in the NSIS bundle" means the hook forwarder was not packaged).' }

$bundle = Join-Path $Root 'src-tauri\target\release\bundle\nsis'
$installer = Get-ChildItem -Path $bundle -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { Die "no installer (*-setup.exe) in $bundle." }

Say "running $($installer.Name) (silent, per-user)"
$process = Start-Process -FilePath $installer.FullName -ArgumentList '/S' -Wait -PassThru
if ($process.ExitCode -ne 0) { Die "the installer exited with code $($process.ExitCode)." }

Say 'installed. Start DCTerminalv2 from the Start menu.'
