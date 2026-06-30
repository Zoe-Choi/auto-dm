$ErrorActionPreference = "Stop"
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)

if (-not (Test-Path -LiteralPath ".venv\Scripts\python.exe")) {
    python -m venv .venv
}

& .\.venv\Scripts\python.exe -m pip install -e ".[dev]"
& .\.venv\Scripts\python.exe -m auto_dm.cli worker --limit 50 --interval 5