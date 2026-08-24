param(
  [Parameter(Position = 0, Mandatory = $true)]
  [string] $Destination
)

$ErrorActionPreference = "Stop"

$repo = if ($env:KIRI_REPO) { $env:KIRI_REPO } else { "ChloeVPin/kiri" }
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$name = Split-Path $destinationPath -Leaf
$feedUrl = "https://github.com/$repo/releases/latest/download/RELEASES.json"
$feed = Invoke-RestMethod -Uri $feedUrl -Headers @{ "User-Agent" = "create-kiri-app" }
$platform = "windows-x86_64"
$asset = $feed.platforms.$platform
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

  $unpack = Join-Path $stage "unpack"
  Expand-Archive -Path $archive -DestinationPath $unpack
  $host = Get-ChildItem -Path $unpack -Filter "kiri-host.exe" -Recurse | Select-Object -First 1
  if (-not $host) { throw "Release archive did not contain kiri-host.exe" }

  New-Item -ItemType Directory -Force -Path (Join-Path $destinationPath "bin") | Out-Null
  New-Item -ItemType Directory -Force -Path (Join-Path $destinationPath "frontend") | Out-Null
  Copy-Item $host.FullName (Join-Path $destinationPath "bin\kiri-host.exe")

  $starterBase = "https://raw.githubusercontent.com/$repo/main/examples/starter"
  foreach ($file in @("index.html", "kiri.js", "kiri.svg")) {
    Invoke-WebRequest -Uri "$starterBase/$file" -OutFile (Join-Path $destinationPath "frontend\$file") -Headers @{ "User-Agent" = "create-kiri-app" }
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
