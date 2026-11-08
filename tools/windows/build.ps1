# Builds the Windows app into dist\windows (git-ignored): one BranchBooks.exe, with WebView2's loader
# and the C runtime linked in. Needs Rust and Visual Studio Build Tools (README, "Setup").
#   tools\windows\build.ps1
$ErrorActionPreference = 'Stop'
$triple = 'x86_64-pc-windows-msvc'
cargo build --release --locked -p branchbooks-ui --features desktop --target $triple
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist\windows | Out-Null
Copy-Item "target\$triple\release\branchbooks-ui.exe" dist\windows\BranchBooks.exe
Get-ChildItem dist\windows
