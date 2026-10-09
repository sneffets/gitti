# Installs gitti + gitty for the current user: binaries into %LOCALAPPDATA%\Programs\gitti
# (added to the user PATH), a Start menu shortcut for gitti, then refreshes the Windows icon
# cache (Explorer/taskbar otherwise keep showing an old icon of a rebuilt exe).
#   scripts\install-windows.cmd   (wrapper, works with scripts disabled by the execution policy)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Invoke-Cargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo $args failed" }
}

# Icon files from assets/icon/*.svg (rewrites only what changed, so usually no rebuild).
Invoke-Cargo run --release -q -p gitti-app --example make_icons
Invoke-Cargo build --release -p gitti-app -p gitty

$dest = Join-Path $env:LOCALAPPDATA 'Programs\gitti'
New-Item -ItemType Directory -Force $dest | Out-Null
foreach ($exe in 'gitti.exe', 'gitty.exe') {
    try {
        Copy-Item "target\release\$exe" (Join-Path $dest $exe) -Force
    } catch {
        throw "cannot replace $dest\$exe - is it still running? ($_)"
    }
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dest) {
    $newPath = if ($userPath) { "$userPath;$dest" } else { $dest }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "added $dest to the user PATH (new terminals pick it up)"
}

$programs = [Environment]::GetFolderPath('Programs')
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path $programs 'gitti.lnk'))
$link.TargetPath = Join-Path $dest 'gitti.exe'
$link.WorkingDirectory = $env:USERPROFILE
$link.IconLocation = "$(Join-Path $dest 'gitti.exe'),0"
$link.Description = 'gitti - git GUI'
$link.Save()

# Rebuilds the shell's icon cache; pinned taskbar icons may still need an unpin + pin.
$ie4uinit = Join-Path $env:SystemRoot 'System32\ie4uinit.exe'
if (Test-Path $ie4uinit) { & $ie4uinit -show }

Write-Host "installed gitti + gitty into $dest, Start menu entry 'gitti'"
