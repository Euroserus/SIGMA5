# Publishes a SIGMA5 release to GitHub, which installed copies then pick up
# through the built-in updater.
#
#   1. Bump `version` in Cargo.toml and `CODENAME` in src/install.rs
#   2. Write release notes to a file (Markdown)
#   3. powershell -ExecutionPolicy Bypass -File scripts/release.ps1 -Notes notes.md
#
# The release title must be `SIGMA5 <version> <Codename in curly quotes>`; the
# updater reads the codename from it. The asset must be named sigma5.exe.
param(
    [Parameter(Mandatory = $true)][string]$Notes,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$codename = (Select-String -Path src/install.rs -Pattern 'pub const CODENAME: &str = "([^"]+)"').Matches[0].Groups[1].Value
$tag = "v$version"
# Curly quotes via char codes: PowerShell treats literal “ ” as string quotes.
$title = "SIGMA5 $version " + [char]0x201C + $codename + [char]0x201D
Write-Host "Releasing $title ($tag)"

if (git tag --list $tag) { throw "Tag $tag already exists. Bump the version in Cargo.toml first." }
if (git status --porcelain) { throw "Commit your changes before releasing." }
if (-not (Test-Path $Notes)) { throw "Notes file '$Notes' not found." }

cargo test --release
if ($LASTEXITCODE) { throw "Tests failed." }
cargo build --release
if ($LASTEXITCODE) { throw "Build failed." }

$exe = "target/release/sigma5.exe"
$hash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
Write-Host "sigma5.exe  $([math]::Round((Get-Item $exe).Length / 1KB)) KB  sha256 $hash"
if ($DryRun) { Write-Host "Dry run: not publishing."; return }

# Publish only once main is on GitHub, so the release tag gets this commit.
git push
if ($LASTEXITCODE) { throw "git push failed (pull or rebase first). Nothing was published." }
$head = git rev-parse HEAD
gh release create $tag $exe --target $head --title $title --notes-file $Notes
if ($LASTEXITCODE) { throw "gh release create failed." }
Write-Host "Published. Installed copies will offer the update within 12 hours (or via 'Check for Updates')."
