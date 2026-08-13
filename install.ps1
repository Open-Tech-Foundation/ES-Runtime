# ES Runtime installer for Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.ps1 | iex
#
# Installs both binaries by default - `esrun`, the server runtime, and `esdev`,
# the development toolchain - into $HOME\.es-runtime\bin, verifying each SHA-256
# checksum when the release ships one.
#
#   $env:ES_RUNTIME_ONLY = 'esrun'    just the server runtime (servers, CI)
#   $env:ES_RUNTIME_ONLY = 'esdev'    just the development binary
#
#   $env:ESRUN_VERSION = '0.24.0'     pin one binary; '0.24.0' or 'esrun@0.24.0'
#   $env:ESDEV_VERSION = '0.1.0'      likewise
#   $env:ES_RUNTIME_INSTALL           install prefix (default $HOME\.es-runtime)
#
# Releases are tagged per binary - `esrun@0.24.0`, `esdev@0.1.0` - so the version
# for each is resolved from the newest tag carrying *its* prefix. Asking GitHub
# for /releases/latest would return whichever binary was published most
# recently, which is how this script used to try to download an esrun archive
# from an esdev release.
#
# `-only` is an environment variable rather than a parameter because the
# documented entry point is `irm ... | iex`, which has no way to pass arguments.

$ErrorActionPreference = 'Stop'

$Repo = 'Open-Tech-Foundation/ES-Runtime'
# ESRUN_INSTALL is the pre-0.25 name, still honoured so an existing setup that
# sets it keeps working.
$InstallDir =
  if ($env:ES_RUNTIME_INSTALL) { $env:ES_RUNTIME_INSTALL }
  elseif ($env:ESRUN_INSTALL) { $env:ESRUN_INSTALL }
  else { Join-Path $HOME '.es-runtime' }
$BinDir = Join-Path $InstallDir 'bin'
$LegacyBinDir = Join-Path (Join-Path $HOME '.esrun') 'bin'

# --- what to install --------------------------------------------------------
$Bins = @('esrun', 'esdev')
if ($env:ES_RUNTIME_ONLY) {
  if ($env:ES_RUNTIME_ONLY -notin @('esrun', 'esdev')) {
    throw "unknown ES_RUNTIME_ONLY value: $($env:ES_RUNTIME_ONLY) (expected esrun or esdev)"
  }
  $Bins = @($env:ES_RUNTIME_ONLY)
}

# --- detect platform --------------------------------------------------------
# Release assets are named `<bin>-<os>-<arch>` by the otf-release tool
# (see release.toml), e.g. `esrun-windows-x86-64.zip`.
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { 'x86-64' }
  'ARM64' { 'arm64' }
  default { throw "unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)" }
}
$target = "windows-$arch"

# Binaries with no release asset for this platform. `esdev` builds for
# windows-arm64 and `esrun` does not (release.toml), so the default install of
# both has to say which one is missing and why - a bare download failure would
# read as a broken installer.
$Unavailable = @{}
if ($target -eq 'windows-arm64') {
  $Unavailable['esrun'] = 'esrun is not published for windows-arm64 yet (esdev is). Deploy from an x64 machine, or run esrun under emulation.'
}

# The newest release tag for one binary, e.g. `esrun@0.24.0`.
#
# Tags come back newest-first, so the first match wins. `esrun` also answers to
# the pre-0.24 `v<version>` tags (release.toml's legacy_tag_formats), which is
# why its pattern accepts both.
function Get-LatestTag($bin) {
  $pattern = if ($bin -eq 'esrun') { '^(esrun@|v[0-9])' } else { "^$bin@" }
  $releases = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases?per_page=100"
  ($releases | ForEach-Object { $_.tag_name } | Where-Object { $_ -match $pattern } | Select-Object -First 1)
}

function Install-One($bin) {
  $pinned = [Environment]::GetEnvironmentVariable("$($bin.ToUpper())_VERSION")
  if ($pinned) {
    # A bare '0.24.0' is the friendly spelling; a full 'esrun@0.24.0' or a
    # legacy 'v0.23.0' is passed through as written.
    $tag = if ($pinned -match '@' -or $pinned -match '^v') { $pinned } else { "$bin@$pinned" }
  } else {
    $tag = Get-LatestTag $bin
    if (-not $tag) { throw "could not find a released $bin (set $($bin.ToUpper())_VERSION)" }
  }

  $name = "$bin-$target"
  $url = "https://github.com/$Repo/releases/download/$tag/$name.zip"
  $shown = if ($tag -match '@') { $tag.Split('@')[1] } else { $tag }

  Write-Host "Installing $bin $shown ($target)" -ForegroundColor Cyan
  Write-Host "  from $url" -ForegroundColor DarkGray

  $tmp = Join-Path $env:TEMP ("es-runtime-" + [System.Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tmp | Out-Null
  try {
    $zip = Join-Path $tmp "$name.zip"
    try {
      Invoke-WebRequest -Uri $url -OutFile $zip
    } catch {
      throw "download failed - is there a $bin release asset for $target?"
    }

    # Checksums, when present, live in one `checksums.txt` per release
    # (`<hash>  <archive>` lines); pull out the line for our archive and verify
    # it. A release without a checksums.txt is not fatal - verification is
    # skipped.
    $sumFile = Join-Path $tmp 'checksums.txt'
    $sumsUrl = "https://github.com/$Repo/releases/download/$tag/checksums.txt"
    $line = $null
    try {
      Invoke-WebRequest -Uri $sumsUrl -OutFile $sumFile
      $line = Get-Content $sumFile | Where-Object { $_ -match "  $([regex]::Escape($name)).zip$" } | Select-Object -First 1
    } catch {}
    if ($line) {
      $expected = (($line -split '\s+')[0]).ToLower()
      $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
      if ($expected -ne $actual) { throw 'checksum verification failed' }
      Write-Host '  checksum verified' -ForegroundColor DarkGray
    } else {
      Write-Host '  no checksums.txt for this release - skipping verification' -ForegroundColor DarkGray
    }

    # The archive holds the binary at its root.
    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item (Join-Path $tmp "$bin.exe") (Join-Path $BinDir "$bin.exe") -Force
    Write-Host "  installed to $BinDir\$bin.exe" -ForegroundColor DarkGray
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }
}

# A binary with no asset for this platform is skipped with its reason when the
# user asked for everything, and is a hard error when they asked for it by name.
$installed = @()
foreach ($bin in $Bins) {
  if ($Unavailable.ContainsKey($bin)) {
    if ($Bins.Count -eq 1) { throw $Unavailable[$bin] }
    Write-Host "skipping $bin - $($Unavailable[$bin])" -ForegroundColor Yellow
    continue
  }
  Install-One $bin
  $installed += $bin
}
if ($installed.Count -eq 0) { throw "nothing to install for $target" }

Write-Host ''
Write-Host "Installed $($installed -join ', ') to $BinDir"

# The pre-0.25 location. Left in place - removing binaries this script did not
# put there is not its call - but a stale copy earlier in PATH would shadow what
# was just installed, which is the confusing failure worth naming.
if ((Test-Path $LegacyBinDir) -and ($LegacyBinDir -ne $BinDir)) {
  Write-Host ''
  Write-Host "note: an older install remains at $LegacyBinDir" -ForegroundColor Yellow
  Write-Host '  It is not removed automatically. If it comes first in PATH it will' -ForegroundColor DarkGray
  Write-Host '  shadow the binaries above - remove it, or drop its PATH entry.' -ForegroundColor DarkGray
}

# Add to the user PATH if it isn't already there.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $BinDir) {
  [Environment]::SetEnvironmentVariable('Path', "$BinDir;$userPath", 'User')
  Write-Host "Added $BinDir to your user PATH - restart your shell to pick it up."
}
foreach ($bin in $installed) {
  Write-Host "Run '$bin --version' to verify." -ForegroundColor DarkGray
}
