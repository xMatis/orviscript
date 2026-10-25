$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$metadata = cargo metadata --manifest-path (Join-Path $repositoryRoot "Cargo.toml") --no-deps --format-version 1 | ConvertFrom-Json
$packages = @($metadata.packages | Where-Object { $_.name -like "orvi-*" })
$versions = @($packages.version | Sort-Object -Unique)

if ($versions.Count -ne 1) {
    throw "All OrviScript crates must use one version. Found: $($versions -join ', ')"
}

$extensionManifest = Get-Content (Join-Path $repositoryRoot "editors/vscode/package.json") -Raw | ConvertFrom-Json
if ($extensionManifest.version -ne $versions[0]) {
    throw "VS Code extension version $($extensionManifest.version) does not match crate version $($versions[0])."
}

$versions[0]
