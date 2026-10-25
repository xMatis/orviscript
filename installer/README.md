# OrviScript distribution

The Windows installer uses a stable Inno Setup `AppId`, so every later build upgrades the same per-user installation. Its version is injected from the Cargo workspace at build time; do not copy or rename the installer script for a new release.

## Local Windows build

Install Rust, Node.js, and Inno Setup 6. The staging script downloads the pinned `@vscode/vsce` version automatically. Then run from the repository root:

```powershell
$staged = ./scripts/release/Stage-Windows.ps1
iscc /DAppVersion=$($staged.Version) /DSourceDir=$($staged.SourceDirectory) /DOutputDir="$PWD/artifacts" installer/windows/orviscript.iss
```

The installer places `orviscript.exe` and `orvi-lsp.exe` in a stable directory, adds that directory to the current user's `PATH`, registers `.orvi` with the bundled OrviScript file icon, includes the VSIX, and removes its own `PATH` entry during uninstall. The optional VS Code task installs or upgrades the bundled extension when a standard VS Code installation is detected.

For code signing, configure an Inno Setup signing tool and pass `/DSignToolName=NAME`. Secrets and signing commands belong in CI, not in this repository.

## Release contract

All Rust crates and `editors/vscode/package.json` must have the same semantic version. `scripts/release/Get-OrviVersion.ps1` enforces this. A `vX.Y.Z` tag must match that version; release CI builds archives, a VSIX, the Windows installer, and SHA-256 files. Future releases change the project version and create a matching tag—the installer source stays unchanged.
