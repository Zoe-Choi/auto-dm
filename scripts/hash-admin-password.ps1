param(
    [Parameter(Mandatory = $true)]
    [string]$Password
)

. "$PSScriptRoot\cargo-env.ps1"
cargo run --release --quiet -- hash-password $Password
