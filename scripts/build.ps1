param(
    [switch]$Release,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\dist')
)
$ErrorActionPreference = 'Stop'
$project = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location -LiteralPath $project
try {
    if (-not (Test-Path -LiteralPath 'native\jmt.dll')) {
        throw 'Отсутствует native/jmt.dll из исходного EXBO launcher/native.'
    }
    if (-not ((rustup target list --installed) -contains 'i686-pc-windows-msvc')) {
        rustup target add i686-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { throw 'Не удалось установить x86 target Rust.' }
    }
    rustc --target i686-pc-windows-msvc --edition 2024 -O -C debuginfo=0 -C strip=symbols -C target-feature=+crt-static native\hwid_bridge.rs -o native\exbo-hwid.exe
    if ($LASTEXITCODE -ne 0) { throw 'Ошибка сборки адаптера HWID.' }
    $cargoArgs = @('build', '--locked')
    $profile = 'debug'
    if ($Release) { $cargoArgs += '--release'; $profile = 'release' }
    cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Ошибка сборки лаунчера.' }
    $destination = [IO.Path]::GetFullPath($OutputDirectory)
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Copy-Item -LiteralPath (Join-Path $project "target\x86_64-pc-windows-msvc\$profile\ExboLauncherNew.exe") -Destination $destination
    Copy-Item -LiteralPath (Join-Path $project 'README.md') -Destination $destination -Force
    Copy-Item -LiteralPath (Join-Path $project 'LICENSE') -Destination $destination -Force
    Copy-Item -LiteralPath (Join-Path $project 'docs') -Destination $destination -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $project 'LICENSES') -Destination $destination -Recurse -Force
    Write-Output "Сборка готова: $destination"
} finally {
    Pop-Location
}
