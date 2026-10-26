param(
    [int]$Warmup = 3,
    [int]$Runs = 10,
    [string]$OutputDirectory = "artifacts/benchmarks"
)

$ErrorActionPreference = "Stop"
$repository = Resolve-Path (Join-Path $PSScriptRoot "../..")
$hyperfine = Get-Command hyperfine -ErrorAction SilentlyContinue
if (-not $hyperfine) {
    throw "hyperfine is not installed. With Rust 1.95 install it with: cargo install hyperfine --version 1.20.0 --locked"
}

Push-Location $repository
try {
    cargo build --workspace --release
    if ($LASTEXITCODE -ne 0) { throw "release build failed" }

    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $binary = Join-Path $repository "target/release/orviscript.exe"
    if (-not (Test-Path $binary)) {
        $binary = Join-Path $repository "target/release/orviscript"
    }

    $commands = @(
        "`"$binary`" run benchmarks/startup.orvi",
        "`"$binary`" run benchmarks/sync_loop.orvi",
        "`"$binary`" run benchmarks/async_10000_tasks.orvi",
        "`"$binary`" run benchmarks/async_timers.orvi"
    )
    $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $json = Join-Path $OutputDirectory "orviscript-$timestamp.json"
    $markdown = Join-Path $OutputDirectory "orviscript-$timestamp.md"

    & $hyperfine.Source --warmup $Warmup --runs $Runs --export-json $json --export-markdown $markdown @commands
    if ($LASTEXITCODE -ne 0) { throw "hyperfine failed" }

    Write-Output "Benchmark JSON: $json"
    Write-Output "Benchmark Markdown: $markdown"
}
finally {
    Pop-Location
}
