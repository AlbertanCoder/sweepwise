# Opens the real Sweepwise app once per screen, saves a picture of each, and checks the screens reported no errors.
# Uses its own empty records folder (SWEEPWISE_DATA), so the person's real Sweepwise history and settings are untouched.
# Output: tools/screens/<screen>.png and tools/screens/RESULT.json ({ ui_errors, errors, shots }).
param(
    [string[]]$Screens = @('terms', 'welcome1', 'welcome2', 'welcome3', 'keyguide', 'home', 'boost', 'email', 'security', 'help', 'support', 'about', 'settings', 'supportcard', 'light:home', 'light:settings'),
    [int]$WaitSeconds = 7
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'src-tauri\target\release\sweepwise.exe'
$outDir = Join-Path $PSScriptRoot 'screens'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$data = Join-Path $env:TEMP ('sweepwise-screens-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$dark = Join-Path $data 'dark'
$light = Join-Path $data 'light'
New-Item -ItemType Directory -Force -Path $dark, $light | Out-Null
# The light-theme records folder starts with the welcome already done and the Light theme chosen.
Set-Content -Path (Join-Path $light 'settings.json') -Value '{ "theme": "light", "welcome_done": true, "terms_version": 1 }' -Encoding ASCII

$shots = @()
foreach ($item in $Screens) {
    $useLight = $item.StartsWith('light:')
    $screen = $item -replace '^light:', ''
    $env:SWEEPWISE_DATA = $(if ($useLight) { $light } else { $dark })
    $env:SWEEPWISE_SCREEN = $screen
    Get-Process sweepwise -ErrorAction SilentlyContinue | ForEach-Object { $_.CloseMainWindow() | Out-Null }
    Start-Sleep -Milliseconds 800
    $p = Start-Process $exe -PassThru
    Start-Sleep -Seconds $WaitSeconds
    $name = $(if ($useLight) { "light-$screen" } else { $screen })
    $png = Join-Path $outDir "$name.png"
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'capture.ps1') -Out $png -WaitSeconds 10 | Out-Null
    if (Test-Path $png) { $shots += $name }
    $p.CloseMainWindow() | Out-Null
    if (-not $p.WaitForExit(8000)) { $p.Kill() }
}
Remove-Item Env:\SWEEPWISE_DATA, Env:\SWEEPWISE_SCREEN -ErrorAction SilentlyContinue

$errors = @()
foreach ($d in $dark, $light) {
    $log = Join-Path $d 'ui_errors.log'
    if (Test-Path $log) { $errors += @(Get-Content $log -Encoding UTF8 | Where-Object { $_.Trim() }) }
}
$result = [ordered]@{ made = (Get-Date).ToString('s'); ui_errors = $errors.Count; errors = $errors; shots = $shots }
# UTF-8 without a byte-order mark, so any JSON reader accepts it.
[IO.File]::WriteAllText((Join-Path $outDir 'RESULT.json'), (ConvertTo-Json -InputObject $result -Depth 4), (New-Object Text.UTF8Encoding $false))
Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
"shots: $($shots.Count) of $($Screens.Count); screen errors: $($errors.Count)"
$errors | Select-Object -First 10
