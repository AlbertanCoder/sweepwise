# Packs the Windows release program into an .msix for the Microsoft Store (the Store signs it, so it is left unsigned here).
# Usage: build_msix.ps1 -Exe <path to sweepwise.exe> -IdentityName <Package/Identity/Name> -Publisher <CN=...> -PublisherDisplay <name> [-Version 0.1.0.0] [-Out <folder>]
# The three identity values come from Partner Center (Product identity) after the name Sweepwise is reserved.
param(
  [Parameter(Mandatory)][string]$Exe,
  [Parameter(Mandatory)][string]$IdentityName,
  [Parameter(Mandatory)][string]$Publisher,
  [Parameter(Mandatory)][string]$PublisherDisplay,
  [string]$Version = '0.1.0.0',
  [string]$Out = (Join-Path $PSScriptRoot 'out')
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+\.0$') { throw "Version must look like 0.1.0.0 (the Store keeps the last part at 0), got $Version" }
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" | Sort-Object FullName | Select-Object -Last 1
if (-not $sdk) { throw 'makeappx.exe not found: install the Windows 10/11 SDK.' }
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$layout = Join-Path $Out 'layout'
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $layout 'Assets') | Out-Null
Copy-Item $Exe (Join-Path $layout 'sweepwise.exe')
foreach ($n in 'StoreLogo', 'Square44x44Logo', 'Square150x150Logo') { Copy-Item (Join-Path $repo "src-tauri\icons\$n.png") (Join-Path $layout "Assets\$n.png") }
$x = Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml.template') -Raw
$x = $x.Replace('{{IDENTITY_NAME}}', $IdentityName).Replace('{{PUBLISHER}}', $Publisher).Replace('{{PUBLISHER_DISPLAY}}', $PublisherDisplay).Replace('{{VERSION}}', $Version)
[IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $x, (New-Object Text.UTF8Encoding $false))
$pkg = Join-Path $Out "Sweepwise_${Version}_x64.msix"
if (Test-Path $pkg) { Remove-Item $pkg -Force }
& $sdk.FullName pack /d $layout /p $pkg /o
if ($LASTEXITCODE -ne 0) { throw "makeappx pack failed ($LASTEXITCODE)" }
Write-Host "built $pkg"
Get-FileHash $pkg -Algorithm SHA256 | ForEach-Object { "$($_.Hash.ToLower()) *$(Split-Path $pkg -Leaf)" }
