# Builds the QuotaDock Windows installer (NSIS, per-user) into target\release\bundle\nsis.
#
# Needs Rust (MSVC), Node with ui\node_modules installed, and tauri-cli 2.12.4 in target\tools:
#   cargo install tauri-cli --version 2.12.4 --locked --root target\tools
# On first use the bundler downloads NSIS 3.13 and nsis_tauri_utils.dll 0.5.3 (SHA-1 checked) into
# target\.tauri (bundle.useLocalToolsDir). If its download times out ("timeout: global"), fetch the
# same two URLs (see tauri-bundler 2.10.4 nsis/mod.rs) with curl, check the SHA-1 values listed there,
# unzip nsis-3.13.zip into target\.tauri, rename nsis-3.13 to NSIS and put the DLL in
# NSIS\Plugins\x86-unicode\additional. Bundle settings live in src-tauri\bundle.windows.json, so a
# plain `cargo build` / `cargo test` does not need the bridge copy or the NSIS files.
#
# usage: powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$cli = Join-Path $root "target\tools\bin\cargo-tauri.exe"
if (-not (Test-Path $cli)) { throw "tauri-cli not found at $cli (see the header of this script)" }

# Keep local paths (user name, checkout folder) out of the binaries.
$sep = [char]0x1f
$remap = "--remap-path-prefix=$root=quotadock$sep--remap-path-prefix=$env:USERPROFILE=~"

function Invoke-Checked([string]$what, [scriptblock]$block) {
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

# 1. Claude status-line bridge, with the C runtime linked statically: users' machines may lack the
#    VC++ redistributable, and a bridge that cannot start would leave Claude Code's status line blank.
$env:CARGO_ENCODED_RUSTFLAGS = "$remap$sep-Ctarget-feature=+crt-static"
Invoke-Checked "bridge build" { cargo build --release -p quotadock-core --bin quotadock-claude-bridge --target-dir target\bridge-static }
$bridge = "target\bridge-static\release\quotadock-claude-bridge.exe"
if ([Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes((Join-Path $root $bridge))) -match "(?i)vcruntime140") {
    throw "the bridge still imports VCRUNTIME140.dll"
}
New-Item -ItemType Directory -Force src-tauri\binaries | Out-Null
Copy-Item $bridge src-tauri\binaries\quotadock-claude-bridge-x86_64-pc-windows-msvc.exe -Force

# 2. UI (type check, then the production bundle in ui\dist).
Remove-Item Env:CARGO_ENCODED_RUSTFLAGS
Push-Location ui
try {
    Invoke-Checked "ui type check" { node node_modules\typescript\bin\tsc --noEmit -p . }
    Invoke-Checked "ui build" { node node_modules\vite\bin\vite.js build }
} finally { Pop-Location }

# 3. App (release, tauri/custom-protocol) and the NSIS installer.
$env:CARGO_ENCODED_RUSTFLAGS = $remap
Invoke-Checked "tauri build" { & $cli build --bundles nsis --config src-tauri\bundle.windows.json }
Remove-Item Env:CARGO_ENCODED_RUSTFLAGS

$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$setup = "target\release\bundle\nsis\QuotaDock_${version}_x64-setup.exe"
foreach ($f in $setup, "target\release\quotadock-app.exe", "target\release\quotadock-claude-bridge.exe") {
    $item = Get-Item $f
    "{0}  {1,12:N0} B  {2}" -f (Get-FileHash $f -Algorithm SHA256).Hash, $item.Length, $f
}
