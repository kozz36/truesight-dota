param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [string]$GuiExePath = "",
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$OutDir
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$PackagingDir = Split-Path -Parent $ScriptDir
$RepoRoot = Split-Path -Parent $PackagingDir

$CleanVersion = ($Version -replace '^v', '') -replace '-.*$', ''
if ($CleanVersion -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') {
    throw "MSI version must be numeric Major.Minor.Patch (got '$Version' -> '$CleanVersion')"
}

if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Release binary not found at: $ExePath"
}

if ([string]::IsNullOrWhiteSpace($GuiExePath)) {
    $GuiExePath = Join-Path (Split-Path -Parent $ExePath) "truesight-dota-gui.exe"
}
if (-not (Test-Path -LiteralPath $GuiExePath)) {
    throw "Windows GUI binary not found at: $GuiExePath"
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$StageDir = Join-Path $OutDir "stage-windows-x86_64"
if (Test-Path -LiteralPath $StageDir) {
    Remove-Item -LiteralPath $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Path $StageDir -Force | Out-Null

Copy-Item -LiteralPath $ExePath -Destination (Join-Path $StageDir "truesight-dota.exe")
Copy-Item -LiteralPath $GuiExePath -Destination (Join-Path $StageDir "truesight-dota-gui.exe")
Copy-Item -LiteralPath (Join-Path $PackagingDir "gamestate_integration_truesight.cfg") -Destination (Join-Path $StageDir "gamestate_integration_truesight.cfg")
Copy-Item -LiteralPath (Join-Path $ScriptDir "install-gsi.ps1") -Destination (Join-Path $StageDir "install-gsi.ps1")
Copy-Item -LiteralPath (Join-Path $ScriptDir "truesight-dota.ico") -Destination (Join-Path $StageDir "truesight-dota.ico")
Copy-Item -LiteralPath (Join-Path $RepoRoot "LICENSE") -Destination (Join-Path $StageDir "LICENSE")
Copy-Item -LiteralPath (Join-Path $RepoRoot "README.md") -Destination (Join-Path $StageDir "README.md")

$ZipPath = Join-Path $OutDir "truesight-dota-windows-x86_64.zip"
if (Test-Path -LiteralPath $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
}
Compress-Archive -Path (Join-Path $StageDir "*") -DestinationPath $ZipPath -Force

$MsiPath = Join-Path $OutDir "truesight-dota-windows-x86_64.msi"
if (Test-Path -LiteralPath $MsiPath) {
    Remove-Item -LiteralPath $MsiPath -Force
}
$StageFullDir = (Resolve-Path -LiteralPath $StageDir).Path
$WxsPath = Join-Path $ScriptDir "Package.wxs"
wix build $WxsPath -arch x64 -d Version=$CleanVersion -d StageDir=$StageFullDir -o $MsiPath
if ($LASTEXITCODE -ne 0) {
    throw "wix build failed with exit code $LASTEXITCODE"
}

Remove-Item -LiteralPath $StageDir -Recurse -Force
Write-Host "Built Windows release packages:"
Write-Host "  $ZipPath"
Write-Host "  $MsiPath"
