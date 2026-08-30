param(
  [Parameter(Position = 0, Mandatory = $true)]
  [string] $Destination,
  [Parameter(Position = 1)]
  [ValidateSet("starter","starter-vite","blank")]
  [string] $Template = "starter"
)

$ErrorActionPreference = "Stop"

# Support --template flag: create-kiri-app.ps1 --template starter-vite DIR
if ($Destination -eq "--template" -and $Template -ne "starter") {
  # PowerShell already bound $Template as second positional; keep it
} elseif ($Destination -eq "--template") {
  if ($args.Count -ge 1) {
    $Template = $args[0]
    if ($args.Count -ge 2) { $Destination = $args[1] } else { throw "usage: create-kiri-app.ps1 [--template starter|starter-vite|blank] DIR" }
  } else {
    throw "usage: create-kiri-app.ps1 [--template starter|starter-vite|blank] DIR"
  }
}

$repo = if ($env:KIRI_REPO) { $env:KIRI_REPO } else { "ChloeVPin/kiri" }
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$name = Split-Path $destinationPath -Leaf
$feedUrl = "https://github.com/$repo/releases/latest/download/RELEASES.json"
$feed = Invoke-RestMethod -Uri $feedUrl -Headers @{ "User-Agent" = "create-kiri-app" }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64" -or $env:PROCESSOR_ARCHITEW6432 -eq "ARM64") { "aarch64" } else { "x86_64" }
$platform = "windows-$arch"
$asset = $feed.platforms.$platform
if (-not $asset) {
  # Fallback to x86_64 asset for mixed-arch releases
  $platform = "windows-x86_64"
  $asset = $feed.platforms.$platform
}
if (-not $asset) {
  throw "Release manifest has no asset for $platform"
}
if (-not $asset.sha256 -or -not $asset.signature) {
  throw "Release manifest asset is not signed for $platform"
}

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("kiri-app-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
  $archive = Join-Path $stage "kiri.zip"
  Invoke-WebRequest -Uri $asset.url -OutFile $archive -Headers @{ "User-Agent" = "create-kiri-app" }
  $actual = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLowerInvariant()
  if ($actual -ne $asset.sha256.ToLowerInvariant()) {
    throw "Release hash mismatch: expected $($asset.sha256), got $actual"
  }
  Write-Host "    SHA-256 $actual"

  $unpack = Join-Path $stage "unpack"
  Expand-Archive -Path $archive -DestinationPath $unpack
  $hostBinary = Get-ChildItem -Path $unpack -Filter "kiri-host.exe" -Recurse | Select-Object -First 1
  if (-not $hostBinary) { throw "Release archive did not contain kiri-host.exe" }

  New-Item -ItemType Directory -Force -Path (Join-Path $destinationPath "bin") | Out-Null
  New-Item -ItemType Directory -Force -Path (Join-Path $destinationPath "frontend") | Out-Null
  Copy-Item $hostBinary.FullName (Join-Path $destinationPath "bin\kiri-host.exe")

  $scriptDir = Split-Path $MyInvocation.MyCommand.Path -Parent
  $localStarter = Join-Path $scriptDir "..\examples\$Template"
  if (Test-Path "$localStarter\index.html") {
    Copy-Item -Path "$localStarter\*" -Destination (Join-Path $destinationPath "frontend") -Recurse -Force
    Remove-Item -Path (Join-Path $destinationPath "frontend\README.md") -ErrorAction SilentlyContinue
  } else {
    $starterBase = "https://raw.githubusercontent.com/$repo/main/examples/$Template"
    foreach ($file in @("index.html", "kiri.js", "kiri.svg")) {
      Invoke-WebRequest -Uri "$starterBase/$file" -OutFile (Join-Path $destinationPath "frontend\$file") -Headers @{ "User-Agent" = "create-kiri-app" }
    }
  }

  @"
@echo off
cd /d "%~dp0"
bin\kiri-host.exe --frontend frontend
"@ | Set-Content -Encoding ASCII (Join-Path $destinationPath "run.cmd")

  @"
# $name

Edit `frontend\` and run `run.cmd`.

Kiri release: $($feed.version)
"@ | Set-Content (Join-Path $destinationPath "README.md")
  Write-Host "created $destinationPath (Kiri $($feed.version))"
}
finally {
  Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
