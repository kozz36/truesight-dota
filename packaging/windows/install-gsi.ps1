param(
    [string]$DotaDir = "",
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$CfgSource = Join-Path $ScriptDir "gamestate_integration_truesight.cfg"
if (-not (Test-Path -LiteralPath $CfgSource)) {
    $ParentCfg = Join-Path (Split-Path -Parent $ScriptDir) "gamestate_integration_truesight.cfg"
    if (Test-Path -LiteralPath $ParentCfg) {
        $CfgSource = $ParentCfg
    }
}

if (-not (Test-Path -LiteralPath $CfgSource)) {
    throw "Could not locate gamestate_integration_truesight.cfg next to install-gsi.ps1."
}

function Get-SteamLibraryRoots {
    $roots = New-Object System.Collections.Generic.List[string]
    try {
        $steamReg = Get-ItemProperty -Path "HKCU:\Software\Valve\Steam" -Name SteamPath -ErrorAction SilentlyContinue
        if ($steamReg -and $steamReg.SteamPath) {
            $roots.Add([string]$steamReg.SteamPath)
        }
    } catch {}

    foreach ($candidate in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if ($candidate) {
            $roots.Add((Join-Path $candidate "Steam"))
        }
    }

    $expanded = New-Object System.Collections.Generic.List[string]
    foreach ($root in ($roots | Select-Object -Unique)) {
        if (-not $root) { continue }
        $norm = $root.Replace('/', '\')
        if (-not $expanded.Contains($norm)) {
            $expanded.Add($norm)
        }
        $libVdf = Join-Path $norm "steamapps\libraryfolders.vdf"
        if (Test-Path -LiteralPath $libVdf) {
            $matches = [regex]::Matches((Get-Content -LiteralPath $libVdf -Raw), '"path"\s+"([^"]+)"')
            foreach ($m in $matches) {
                $libPath = $m.Groups[1].Value.Replace('\\', '\').Replace('/', '\')
                if ($libPath -and -not $expanded.Contains($libPath)) {
                    $expanded.Add($libPath)
                }
            }
        }
    }
    return $expanded
}

$TargetCfgRoot = ""
if ($DotaDir) {
    $TargetCfgRoot = Join-Path $DotaDir "game\dota\cfg"
    if (-not (Test-Path -LiteralPath $TargetCfgRoot)) {
        $TargetCfgRoot = $DotaDir
    }
} else {
    foreach ($libRoot in (Get-SteamLibraryRoots)) {
        $cand = Join-Path $libRoot "steamapps\common\dota 2 beta\game\dota\cfg"
        if (Test-Path -LiteralPath $cand) {
            $TargetCfgRoot = $cand
            break
        }
    }
}

if (-not $TargetCfgRoot -or -not (Test-Path -LiteralPath $TargetCfgRoot)) {
    Write-Error @"
Could not find Dota 2's game\dota\cfg directory automatically.
Copy gamestate_integration_truesight.cfg manually to:
  <SteamLibrary>\steamapps\common\dota 2 beta\game\dota\cfg\gamestate_integration\gamestate_integration_truesight.cfg
Or re-run with:
  .\install-gsi.ps1 -DotaDir "D:\SteamLibrary\steamapps\common\dota 2 beta"
"@
    exit 1
}

$GsiDir = Join-Path $TargetCfgRoot "gamestate_integration"
$DestFile = Join-Path $GsiDir "gamestate_integration_truesight.cfg"

if ($DryRun) {
    Write-Host "[DryRun] Would install $CfgSource -> $DestFile"
    exit 0
}

New-Item -ItemType Directory -Path $GsiDir -Force | Out-Null
Copy-Item -LiteralPath $CfgSource -Destination $DestFile -Force

Write-Host "Installed Dota 2 GSI config to:"
Write-Host "  $DestFile"
Write-Host ""
Write-Host "Next step in Steam: right-click Dota 2 -> Properties -> Launch Options -> add:"
Write-Host "  -gamestateintegration"
