param([ValidateSet('build','test','install','clean')][string]$Target = 'build')
$ErrorActionPreference = 'Stop'
if ($Target -eq 'clean') {
    if (Test-Path target) { Remove-Item target -Recurse -Force }
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
