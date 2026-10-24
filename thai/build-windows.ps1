# Builds PdfCraft Thai for Windows x64: the upstream craft-fonts revision plus TH Sarabun PSK and
# Sarabun (thai/fonts), statically linked like the official release. Needs Rust (rustup) and the
# Visual Studio Build Tools (C++ workload). Output: target\x86_64-pc-windows-msvc\release\
# pdfcraft.exe and pdfcraft-cli.exe (where packaging\windows\package.ps1 -SkipBuild looks).
#
#   powershell -ExecutionPolicy Bypass -File thai\build-windows.ps1
#
# thai\package-windows.ps1 builds with this and then makes the installer and the portable zip.

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$CraftFontsRef = '3d9ab3a132ed0eff457e424226a435a901da30d3'
$Target = 'x86_64-pc-windows-msvc'
$Work = Join-Path $Root 'target\thai-fonts'
$Fonts = Join-Path $Work "craft-fonts-$CraftFontsRef"

if (-not (Test-Path (Join-Path $Fonts 'fonts\manifest.txt'))) {
    New-Item -ItemType Directory -Force -Path $Work | Out-Null
    $zip = Join-Path $Work 'craft-fonts.zip'
    Invoke-WebRequest "https://github.com/storytold/craft-fonts/archive/$CraftFontsRef.zip" -OutFile $zip -UseBasicParsing
    Expand-Archive $zip $Work -Force
}

# Add TH Sarabun PSK and Sarabun to the font set: each family folder, and each manifest line not
# there yet (so a font set made by an earlier version of this script gains the newer lines).
$manifest = Join-Path $Fonts 'fonts\manifest.txt'
foreach ($family in 'th-sarabun-psk', 'sarabun') {
    Copy-Item -Recurse -Force (Join-Path $PSScriptRoot "fonts\$family") (Join-Path $Fonts 'fonts')
}
$have = [System.IO.File]::ReadAllLines($manifest) | ForEach-Object { $_.Trim() }
$add = [System.IO.File]::ReadAllLines((Join-Path $PSScriptRoot 'fonts\manifest-additions.txt')) |
    ForEach-Object { $_.Trim() } | Where-Object { $_ -and ($have -notcontains $_) }
if ($add) {
    [System.IO.File]::AppendAllText($manifest, "`n" + ($add -join "`n") + "`n", (New-Object System.Text.UTF8Encoding $false))
}

$env:CRAFT_FONTS_DIR = $Fonts
$env:CRAFT_FONTS_REQUIRED = '1'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
# Fail (rather than warn) if the icon and version information can't be embedded, as releases do.
$env:PDFCRAFT_REQUIRE_WINRES = '1'
if (-not $env:PDFCRAFT_BUILD_SHA) { $env:PDFCRAFT_BUILD_SHA = (git -C $Root rev-parse HEAD) }
if (-not $env:PDFCRAFT_BUILD_DATE) { $env:PDFCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }

Push-Location $Root
try {
    cargo build --release --locked -p pdfcraft -p pdfcraft-cli --target $Target
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
Write-Output "Built: $(Join-Path $TargetDir "$Target\release")"
