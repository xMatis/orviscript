param(
    [int]$Warmup = 3,
    [int]$Runs = 10,
    [string]$OutputDirectory = "artifacts/benchmarks",
    [string]$OrviExecutable = ""
)

$ErrorActionPreference = "Stop"
$repository = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$hyperfine = Get-Command hyperfine -ErrorAction SilentlyContinue
$node = Get-Command node -ErrorAction SilentlyContinue
$python = Get-Command python -ErrorAction SilentlyContinue

if (-not $hyperfine) {
    throw "hyperfine is not installed. With Rust 1.95 install it with: cargo install hyperfine --version 1.20.0 --locked"
}
if (-not $node) { throw "Node.js is not installed or is not available on PATH." }
if (-not $python) { throw "Python is not installed or is not available on PATH." }

if (-not $OrviExecutable) {
    $installed = Get-Command orviscript -ErrorAction SilentlyContinue
    if ($installed) {
        $OrviExecutable = $installed.Source
    }
    else {
        $localBinary = Join-Path $repository "target/release/orviscript.exe"
        if (-not (Test-Path -LiteralPath $localBinary)) {
            $localBinary = Join-Path $repository "target/release/orviscript"
        }
        if (-not (Test-Path -LiteralPath $localBinary)) {
            throw "OrviScript was not found on PATH and no local release binary exists. Run cargo build --release first or pass -OrviExecutable."
        }
        $OrviExecutable = $localBinary
    }
}
$OrviExecutable = (Resolve-Path -LiteralPath $OrviExecutable).Path

Push-Location $repository
try {
    $orviOutput = (& $OrviExecutable run benchmarks/comparison.orvi | Out-String).Trim()
    $nodeOutput = (& $node.Source benchmarks/comparison.js | Out-String).Trim()
    $pythonOutput = (& $python.Source benchmarks/comparison.py | Out-String).Trim()
    if ($orviOutput -ne $nodeOutput -or $orviOutput -ne $pythonOutput) {
        throw "Workload outputs differ: OrviScript='$orviOutput', JavaScript='$nodeOutput', Python='$pythonOutput'."
    }

    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $json = Join-Path $OutputDirectory "language-comparison-$timestamp.json"
    $markdown = Join-Path $OutputDirectory "language-comparison-$timestamp.md"

    $orviCommand = "`"$OrviExecutable`" run benchmarks/comparison.orvi"
    $nodeCommand = "`"$($node.Source)`" benchmarks/comparison.js"
    $pythonCommand = "`"$($python.Source)`" benchmarks/comparison.py"
    $arguments = @(
        "--warmup", $Warmup,
        "--runs", $Runs,
        "--export-json", $json,
        "--export-markdown", $markdown,
        "--command-name", "OrviScript", $orviCommand,
        "--command-name", "JavaScript (Node.js)", $nodeCommand,
        "--command-name", "Python", $pythonCommand
    )

    Write-Output "Verified checksum: $orviOutput"
    Write-Output "OrviScript: $(& $OrviExecutable --version)"
    Write-Output "Node.js: $(& $node.Source --version)"
    Write-Output "Python: $(& $python.Source --version)"
    & $hyperfine.Source @arguments
    if ($LASTEXITCODE -ne 0) { throw "hyperfine failed" }

    Write-Output "Benchmark JSON: $json"
    Write-Output "Benchmark Markdown: $markdown"
}
finally {
    Pop-Location
}
