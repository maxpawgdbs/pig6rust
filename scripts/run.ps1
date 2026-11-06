param(
    [ValidateSet('all','main','cert','logs','jday','ai','web','check-config')]
    [string]$Service = 'all',
    [string]$DataDir = '.'
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
if (Test-Path -LiteralPath (Join-Path $projectRoot '.tools\cargo\bin\cargo.exe')) {
    $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
    $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
    $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
}
& cargo run --locked --offline -- $Service --data-dir $DataDir
exit $LASTEXITCODE
