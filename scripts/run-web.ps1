$ErrorActionPreference = "Stop"
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)

if (-not (Test-Path -LiteralPath ".venv\Scripts\python.exe")) {
    python -m venv .venv
}

& .\.venv\Scripts\python.exe -m pip install -e ".[dev]"
& .\.venv\Scripts\python.exe -m uvicorn main:app --host 127.0.0.1 --port 8000