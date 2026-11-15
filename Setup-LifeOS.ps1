$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
Set-Location $root
Write-Host 'LifeOS setup and release build' -ForegroundColor Magenta

$required = @('node','cargo','rustc')
foreach ($name in $required) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "Required tool not found: $name. Install the official Node.js LTS and Rust MSVC toolchain, then rerun this script."
    }
}
$npm = if (Get-Command 'npm.cmd' -ErrorAction SilentlyContinue) { 'npm.cmd' } else { 'npm' }
if (-not (Get-Command $npm -ErrorAction SilentlyContinue)) {
    throw "Required tool not found: npm. Install Node.js LTS, then rerun this script."
}

Write-Host "Node $(node --version); npm $(& $npm --version); Rust $(rustc --version)"
$rustVersion = [version]((rustc --version) -replace ".* (\d+\.\d+\.\d+).*", "$1")
if ($rustVersion -lt [version]"1.90.0") { throw "Rust 1.90 or newer is required; found $rustVersion." }
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw 'Visual Studio Build Tools not found. Install the C++ build tools workload.' }
$vs = & $vswhere -latest -products '*' -property installationPath
if (-not $vs) { throw 'MSVC Build Tools installation was not detected.' }
$link = Get-ChildItem (Join-Path $vs 'VC\Tools\MSVC') -Filter link.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $link) { throw 'MSVC linker not found. Add the MSVC C++ x64/x86 build tools workload.' }
$wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\*' -ErrorAction SilentlyContinue | Where-Object { $_.name -match 'WebView2' } | Select-Object -First 1
if (-not $wv) { Write-Warning 'WebView2 Runtime was not detected. Tauri may prompt for its bootstrapper during install; install Evergreen WebView2 Runtime if the app does not start.' }
if (-not (Test-Path 'src-tauri\icons\icon.ico')) { throw 'App icon is missing.' }

Write-Host 'Installing project dependencies...' -ForegroundColor Cyan
& $npm ci
if ($LASTEXITCODE -ne 0) { throw 'npm ci failed.' }

Write-Host 'Checking TypeScript and building the interface...' -ForegroundColor Cyan
& $npm run build
if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }

Write-Host 'Formatting and checking Rust...' -ForegroundColor Cyan
Push-Location "$root\src-tauri"
cargo fmt --all
if ($LASTEXITCODE -ne 0) { throw 'cargo fmt failed.' }
cargo clippy --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed.' }
cargo test --all
if ($LASTEXITCODE -ne 0) { throw 'cargo tests failed.' }
& $npm --prefix "$root" run tauri -- build --bundles nsis
if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
Pop-Location

New-Item -ItemType Directory -Force "$root\release", "$root\data", "$root\logs" | Out-Null
$running = Get-Process -Name LifeOS -ErrorAction SilentlyContinue
foreach ($process in $running) {
    [void]$process.CloseMainWindow()
    if (-not $process.WaitForExit(10000)) { throw 'Close LifeOS before replacing the release executable.' }
}
Copy-Item "$root\src-tauri\target\release\lifeos.exe" "$root\release\LifeOS.exe" -Force
$desktop = [Environment]::GetFolderPath('Desktop')
$shortcutPath = Join-Path $desktop 'LifeOS.lnk'
$wsh = New-Object -ComObject WScript.Shell
$shortcut = $wsh.CreateShortcut($shortcutPath)
$shortcut.TargetPath = "$root\release\LifeOS.exe"
$shortcut.WorkingDirectory = $root
$shortcut.IconLocation = "$root\LifeOS.ico,0"
$shortcut.Description = 'LifeOS — локальная панель активности Windows'
$shortcut.Save()
if (-not (Test-Path $shortcutPath)) { throw 'Shortcut creation failed.' }
$check = $wsh.CreateShortcut($shortcutPath)
if ($check.TargetPath -ne "$root\release\LifeOS.exe") { throw "Shortcut target mismatch: $($check.TargetPath)" }
Write-Host "Created $shortcutPath"
Write-Host "EXE: $root\release\LifeOS.exe"
$installer = Get-ChildItem "$root\src-tauri\target\release\bundle\nsis" -Filter "*.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { throw "NSIS installer was not produced." }
Write-Host "Installer: $($installer.FullName)"
Write-Host "DB:  $root\data\lifeos.db (created on first launch)"
Write-Host 'Done.' -ForegroundColor Green
