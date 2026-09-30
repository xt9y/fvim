param([ValidateSet('build','test','install','clean')][string]$Target = 'build')
$ErrorActionPreference = 'Stop'
if ($Target -eq 'clean') {
    if (Test-Path -LiteralPath target) { Remove-Item -LiteralPath target -Recurse -Force }
    $Candidates = @()
    if ($null -ne $env:FVIM_PREFIX) {
        # An explicit packaging prefix confines cleanup to that installation.
        $Prefix = if ($env:FVIM_PREFIX) { $env:FVIM_PREFIX } else { '.' }
        $Candidates += Join-Path $Prefix 'bin/fvim.exe'
    } else {
        foreach ($Base in @($env:USERPROFILE, $env:HOME)) {
            if ($Base) {
                $Candidates += Join-Path $Base '.local/bin/fvim.exe'
                $Candidates += Join-Path $Base 'bin/fvim.exe'
            }
        }
        if ($env:LOCALAPPDATA) { $Candidates += Join-Path $env:LOCALAPPDATA 'fvim/bin/fvim.exe' }
        if ($env:FVIM_CONFIG_DIR) { $Candidates += Join-Path $env:FVIM_CONFIG_DIR 'bin/fvim.exe' }
        $Candidates += Join-Path (Get-Location).Path 'fvim.exe'
        foreach ($Directory in ($env:PATH -split ';')) {
            if ($Directory) { $Candidates += [System.IO.Path]::Combine($Directory.Trim('"'), 'fvim.exe') }
        }
    }
    $Failed = $false
    $Candidates = @($Candidates | Select-Object -Unique)
    foreach ($Binary in $Candidates) {
        try {
            $Item = Get-Item -LiteralPath $Binary -Force -ErrorAction Stop
        } catch [System.Management.Automation.ItemNotFoundException] {
            continue
        } catch {
            Write-Warning "Cannot inspect $Binary; retry with sufficient permissions: $_"
            $Failed = $true
            continue
        }
        if ($Item -and -not $Item.PSIsContainer) {
            try {
                Remove-Item -LiteralPath $Binary -Force -ErrorAction Stop
                Write-Host "Removed $Binary"
            } catch {
                Write-Warning "Cannot remove $Binary. Close any running fvim and retry with sufficient permissions: $_"
                $Failed = $true
            }
        }
    }
    if ($Failed) { exit 1 }
    exit 0
}
if ($Target -eq 'install') {
    if (-not (Test-Path target/release/fvim.exe)) { throw 'Run make before make install.' }
    & ./target/release/fvim.exe --install
    exit $LASTEXITCODE
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $CargoBin = Join-Path $CargoHome 'bin'
    if (-not (Test-Path (Join-Path $CargoBin 'cargo.exe'))) {
        $Arch = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'aarch64' } else { 'x86_64' }
        $Installer = Join-Path ([System.IO.Path]::GetTempPath()) ('fvim-rustup-' + [guid]::NewGuid() + '.exe')
        try {
            Invoke-WebRequest "https://static.rust-lang.org/rustup/dist/$Arch-pc-windows-msvc/rustup-init.exe" -OutFile $Installer
            & $Installer -y --profile minimal
            if ($LASTEXITCODE -ne 0) { throw 'Rust setup failed. Install Visual Studio C++ Build Tools and retry make.' }
        } finally { Remove-Item $Installer -Force -ErrorAction SilentlyContinue }
    }
    $env:PATH = "$CargoBin;$env:PATH"
}
switch ($Target) {
    'build' { & cargo build --release --locked }
    'test' { & cargo test --locked }
}
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
