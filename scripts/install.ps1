# Install motivator from the latest GitHub release (Windows x86_64).
#
#   irm https://raw.githubusercontent.com/gitu/motivator/main/scripts/install.ps1 | iex
#
# Options via environment:
#   $env:MOTIVATOR_VERSION      release to install, e.g. 0.1.0 (default: latest)
#   $env:MOTIVATOR_INSTALL_DIR  target directory (default: %LOCALAPPDATA%\Programs\motivator)
$ErrorActionPreference = 'Stop'

$repo = 'gitu/motivator'
$version = if ($env:MOTIVATOR_VERSION) { $env:MOTIVATOR_VERSION } else { 'latest' }
$installDir = if ($env:MOTIVATOR_INSTALL_DIR) { $env:MOTIVATOR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\motivator' }

$asset = 'motivator-x86_64-windows.zip'
$url = if ($version -eq 'latest') {
    "https://github.com/$repo/releases/latest/download/$asset"
} else {
    "https://github.com/$repo/releases/download/v$($version.TrimStart('v'))/$asset"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "motivator-install-$PID"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    Write-Host "downloading $url"
    Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $asset)
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $installDir -Force
    Write-Host "installed $installDir\motivator.exe"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $installDir) {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
        Write-Host "added $installDir to your user PATH (open a new terminal to pick it up)"
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
