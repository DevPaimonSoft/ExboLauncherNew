param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$DownloadUrl,
    [string]$OutputFile = 'update.json'
)
$ErrorActionPreference = 'Stop'
if (-not $DownloadUrl.StartsWith('https://', [StringComparison]::OrdinalIgnoreCase)) { throw 'Требуется HTTPS URL.' }
$manifest = [ordered]@{
    version = $Version
    url = $DownloadUrl
    sha256 = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash.ToLowerInvariant()
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile), ($manifest | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
Write-Output "Манифест сохранён: $OutputFile"
