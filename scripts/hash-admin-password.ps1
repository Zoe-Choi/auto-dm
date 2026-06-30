param(
    [Parameter(Mandatory = $true)]
    [string]$Password
)

$ErrorActionPreference = "Stop"
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)
& .\.venv\Scripts\python.exe -c "from auto_dm.admin import hash_admin_password; import sys; print(hash_admin_password(sys.argv[1]))" $Password