# Builds the Windows app into dist\windows (git-ignored): one BranchBooks.exe, with WebView2's loader
# and the C runtime linked in. Needs Rust and Visual Studio Build Tools (README, "Setup"); -Installer
# also needs Inno Setup 7 (winget install JRSoftware.InnoSetup).
#   tools\windows\build.ps1                      the exe
#   tools\windows\build.ps1 -Installer           and its installer, BranchBooks-setup.exe
#   tools\windows\build.ps1 -Channel dev -Installer
#                                                a dev app that installs beside the release, with its own books
# No --target: on an x86_64-pc-windows-msvc host, .cargo/config.toml's +crt-static applies anyway, and
# the build shares target\release with local builds instead of keeping a second copy of everything.
param(
    [ValidateSet('', 'dev')] [string] $Channel = '',
    [switch] $Installer,
    [string] $OutputDir = 'dist\windows'
)
$ErrorActionPreference = 'Stop'
$triple = 'x86_64-pc-windows-msvc'
$hostTriple = (rustc -vV | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
if ($hostTriple -ne $triple) { throw "Build on an $triple host (this one is $hostTriple)" }
# The channel is compiled in (src/store.rs, src/update.rs): a dev app keeps its own books, no updates.
if ($Channel) { $env:BRANCHBOOKS_CHANNEL = $Channel } else { Remove-Item Env:BRANCHBOOKS_CHANNEL -ErrorAction SilentlyContinue }
cargo build --release --locked -p branchbooks-ui --features desktop
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist\windows, $OutputDir | Out-Null
Copy-Item "target\release\branchbooks-ui.exe" dist\windows\BranchBooks.exe
if ($Installer) {
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
    $iscc = @(
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
        "$env:ProgramFiles\Inno Setup 7\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $iscc) { throw 'Inno Setup 7 is not installed: winget install JRSoftware.InnoSetup' }
    $out = (Resolve-Path $OutputDir).Path
    $exe = (Resolve-Path dist\windows\BranchBooks.exe).Path
    $defines = @("/DAppVersion=$version", "/DSourceExe=$exe", "/DOutputDir=$out")
    if ($Channel) { $defines += "/DChannel=$Channel" }
    & $iscc /Q @defines tools\windows\branchbooks.iss
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
Get-ChildItem $OutputDir
