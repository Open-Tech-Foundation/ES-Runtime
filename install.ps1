# esrun installer for Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.ps1 | iex
#
# Downloads the latest released `esrun` binary for your platform, verifies its
# SHA-256 checksum, and installs it to $HOME\.esrun\bin. Override the version
# with $env:ESRUN_VERSION = 'v0.1.0' and the install dir with
# $env:ESRUN_INSTALL = 'C:\custom\path'.

$ErrorActionPreference = 'Stop'

$Repo = 'Open-Tech-Foundation/ES-Runtime'
$InstallDir = if ($env:ESRUN_INSTALL) { $env:ESRUN_INSTALL } else { Join-Path $HOME '.esrun' }
$BinDir = Join-Path $InstallDir 'bin'

# --- detect platform --------------------------------------------------------
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { 'x86_64' }
  'ARM64' { 'aarch64' }
  default { throw "unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)" }
}
$target = "$arch-pc-windows-msvc"

# --- resolve version --------------------------------------------------------
$version = $env:ESRUN_VERSION
if (-not $version) {
  $rel = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
  $version = $rel.tag_name
}
if (-not $version) { throw 'could not determine the latest release (set $env:ESRUN_VERSION)' }
$verNoV = $version.TrimStart('v')

$name = "esrun-$verNoV-$target"
$url = "https://github.com/$Repo/releases/download/$version/$name.zip"

Write-Host "Installing esrun $version ($target)" -ForegroundColor Cyan
Write-Host "  from $url" -ForegroundColor DarkGray

# --- download + verify ------------------------------------------------------
$tmp = Join-Path $env:TEMP ("esrun-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp "$name.zip"
  try {
    Invoke-WebRequest -Uri $url -OutFile $zip
  } catch {
    throw "download failed - is there a release asset for $target?"
  }

  $sumFile = "$zip.sha256"
  $haveSum = $true
  try { Invoke-WebRequest -Uri "$url.sha256" -OutFile $sumFile } catch { $haveSum = $false }
  if ($haveSum) {
    $expected = (((Get-Content $sumFile) -split '\s+')[0]).ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw 'checksum verification failed' }
    Write-Host '  checksum verified' -ForegroundColor DarkGray
  }

  # --- install --------------------------------------------------------------
  Expand-Archive -Path $zip -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
  Copy-Item (Join-Path $tmp "$name\esrun.exe") (Join-Path $BinDir 'esrun.exe') -Force

  Write-Host ''
  Write-Host "esrun was installed to $BinDir\esrun.exe"

  # Add to the user PATH if it isn't already there.
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (($userPath -split ';') -notcontains $BinDir) {
    [Environment]::SetEnvironmentVariable('Path', "$BinDir;$userPath", 'User')
    Write-Host "Added $BinDir to your user PATH - restart your shell to pick it up."
  }
  Write-Host "Run 'esrun --version' to verify." -ForegroundColor DarkGray
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
