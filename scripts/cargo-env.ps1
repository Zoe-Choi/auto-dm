# Shared setup for the Rust run scripts. Dot-source it: . "$PSScriptRoot\cargo-env.ps1"
# Adds cargo and MinGW to PATH for this session only; user PATH is not changed.
$ErrorActionPreference = "Stop"
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)

$extraPaths = @(
    (Join-Path $env:USERPROFILE ".cargo\bin"),
    (Join-Path $env:USERPROFILE "tools\mingw64\bin")
)
foreach ($dir in $extraPaths) {
    if (Test-Path -LiteralPath $dir) {
        $env:Path = "$dir;$env:Path"
    }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found. Install Rust with rustup first. See README."
}
if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) {
    throw "gcc not found. Extract MinGW-w64 to %USERPROFILE%\tools\mingw64 or add it to PATH. See README."
}
