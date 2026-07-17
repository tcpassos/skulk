# Demo: start skulkd, then drive it with the skulk CLI. One command to see Skulk
# working on Windows.
#
#   cargo build
#   .\scripts\demo.ps1
#
param([int]$Port = 9000)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$skulkd = Join-Path $root 'target\debug\skulkd.exe'
$skulk = Join-Path $root 'target\debug\skulk.exe'

if (-not (Test-Path $skulkd) -or -not (Test-Path $skulk)) {
    Write-Host "Not built yet. Run:  cargo build" -ForegroundColor Yellow
    exit 1
}

# Throwaway loot store in TEMP so the repo stays clean.
$env:SKULK_LOOT = Join-Path $env:TEMP 'skulk-demo.redb'
Remove-Item $env:SKULK_LOOT -ErrorAction SilentlyContinue

Write-Host "starting skulkd (listening on 127.0.0.1:$Port)..." -ForegroundColor Green
$proc = Start-Process -FilePath $skulkd -PassThru -WindowStyle Hidden -WorkingDirectory $root
Start-Sleep -Milliseconds 900

function Show($label, $cmdArgs) {
    Write-Host "`n> skulk $label" -ForegroundColor Cyan
    & $skulk @cmdArgs
}

try {
    Show 'describe' @('describe')
    Show 'sys.info get' @('sys.info', 'get')
    Show 'net.ports scan target=127.0.0.1 ports=8990-9010 timeout_ms=200' `
        @('net.ports', 'scan', 'target=127.0.0.1', 'ports=8990-9010', 'timeout_ms=200')
    Show 'net.services detect target=127.0.0.1 ports=9000' `
        @('net.services', 'detect', 'target=127.0.0.1', 'ports=9000')
    Show 'loot' @('loot')
}
finally {
    Write-Host "`nstopping skulkd." -ForegroundColor Green
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item $env:SKULK_LOOT -ErrorAction SilentlyContinue
}
