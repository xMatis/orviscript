param(
    [string]$OutputDirectory = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$version = & (Join-Path $PSScriptRoot "Get-OrviVersion.ps1")
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repositoryRoot "artifacts/windows-x64"
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)

if (-not $SkipBuild) {
    cargo build --manifest-path (Join-Path $repositoryRoot "Cargo.toml") --release --locked --bin orviscript --bin orvi-lsp | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Cargo release build failed." }
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
Copy-Item (Join-Path $repositoryRoot "target/release/orviscript.exe") $OutputDirectory
Copy-Item (Join-Path $repositoryRoot "target/release/orvi-lsp.exe") $OutputDirectory
Copy-Item (Join-Path $repositoryRoot "README.md") $OutputDirectory
Copy-Item (Join-Path $repositoryRoot "LICENSE") $OutputDirectory
Copy-Item (Join-Path $repositoryRoot "assets/orviscript-file-icon.ico") $OutputDirectory

$vsixPath = Join-Path $OutputDirectory "orviscript-vscode.vsix"
Push-Location (Join-Path $repositoryRoot "editors/vscode")
try {
    npx --yes @vscode/vsce@4.0.0 package --out $vsixPath | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "VS Code extension packaging failed." }
}
finally {
    Pop-Location
}

Write-Output ([pscustomobject]@{
    Version = $version
    SourceDirectory = $OutputDirectory
})
