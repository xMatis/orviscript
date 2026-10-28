
param(
    [int]$Warmup = 5,
    [int]$Runs = 30,
    [string]$OutputDirectory = "artifacts/benchmarks"
)

$ErrorActionPreference = "Stop"
$repository = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path

$hyperfine = Get-Command hyperfine -ErrorAction SilentlyContinue
$orvi = Get-Command orviscript -ErrorAction SilentlyContinue
$node = Get-Command node -ErrorAction SilentlyContinue
$python = Get-Command py -ErrorAction SilentlyContinue

if (-not $hyperfine) {
    throw "hyperfine is not installed. Run: cargo install hyperfine --locked"
}
if (-not $orvi) {
    throw "OrviScript is not installed or is not available on PATH."
}
if (-not $node) {
    throw "Node.js is not installed or is not available on PATH."
}
if (-not $python) {
    throw "Python is not installed or is not available on PATH."
}

if ($Warmup -lt 0) {
    throw "Warmup must be >= 0."
}
if ($Runs -lt 1) {
    throw "Runs must be >= 1."
}

Push-Location $repository

try {
    $orviOutput = (& orviscript run benchmarks/comparison.orvi | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "OrviScript verification failed."
    }

    $nodeOutput = (& node benchmarks/comparison.js | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Node.js verification failed."
    }

    $pythonOutput = (& py benchmarks/comparison.py | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Python verification failed."
    }

    if (
        $orviOutput -ne $nodeOutput -or
        $orviOutput -ne $pythonOutput
    ) {
        throw "Workload outputs differ: OrviScript='$orviOutput', JavaScript='$nodeOutput', Python='$pythonOutput'."
    }

    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

    $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $json = Join-Path $OutputDirectory "language-comparison-$timestamp.json"
    $markdown = Join-Path $OutputDirectory "language-comparison-$timestamp.md"

    $orviCommand = "orviscript run benchmarks/comparison.orvi"
    $nodeCommand = "node benchmarks/comparison.js"
    $pythonCommand = "py benchmarks/comparison.py"

    $arguments = @(
        "--shell=none",
        "--warmup", $Warmup,
        "--runs", $Runs,
        "--export-json", $json,
        "--export-markdown", $markdown,
        "--command-name", "OrviScript", $orviCommand,
        "--command-name", "JavaScript (Node.js)", $nodeCommand,
        "--command-name", "Python", $pythonCommand
    )

    Write-Output "Verified checksum: $orviOutput"
    Write-Output "OrviScript: $(& orviscript --version)"
    Write-Output "Node.js: $(& node --version)"
    Write-Output "Python: $(& py --version)"

    & $hyperfine.Source @arguments

    if ($LASTEXITCODE -ne 0) {
        throw "hyperfine failed"
    }

    Write-Output ""
    Write-Output "Benchmark completed successfully."
    Write-Output "Benchmark JSON: $json"
    Write-Output "Benchmark Markdown: $markdown"
}
finally {
    Pop-Location
}
