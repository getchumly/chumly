# End-to-end tests against a fresh local server: the CI job `e2e`, and the
# same by hand from the repository root: `pwsh .github/scripts/e2e.ps1`.
# Needs port 17777 free. Builds the server, starts it in a new temporary
# folder (no users, so the owner setup code comes from its log), runs
# `end_to_end` and `screen_stress`, stops it, then runs
# `reconnects_after_server_restart` (that test starts the server itself).

$ErrorActionPreference = 'Stop'

function Invoke-Checked([string] $what, [scriptblock] $command) {
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)" }
}

function Show-ServerLog([string] $dir) {
    foreach ($name in 'server.out', 'server.err') {
        $path = Join-Path $dir $name
        if (Test-Path $path) {
            Write-Host "----- $name -----"
            Get-Content $path | Write-Host
        }
    }
}

Invoke-Checked 'building the server' { cargo build -p server --locked }
$exe = if ($IsWindows) { 'chumly-server.exe' } else { 'chumly-server' }
$bin = Join-Path (Resolve-Path 'target/debug') $exe

$dir = Join-Path ([IO.Path]::GetTempPath()) "chumly-e2e-$([Guid]::NewGuid().ToString('N').Substring(0, 8))"
New-Item -ItemType Directory $dir | Out-Null
$config = Join-Path $dir 'server.toml'
Set-Content $config "bind = `"127.0.0.1:17777`"`npassword = `"secret`""
Write-Host "test server folder: $dir"

# Empty input: the server's console sees the end of it and stops reading.
$stdin = Join-Path $dir 'stdin.txt'
New-Item -ItemType File $stdin | Out-Null
$server = Start-Process -FilePath $bin -ArgumentList '--config', $config -PassThru -NoNewWindow `
    -RedirectStandardInput $stdin `
    -RedirectStandardOutput (Join-Path $dir 'server.out') -RedirectStandardError (Join-Path $dir 'server.err')
try {
    # The log is colored; strip the escape codes before looking for the code.
    $code = $null
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $code) {
        if ($server.HasExited) { Show-ServerLog $dir; throw 'the server exited on startup' }
        if ((Get-Date) -gt $deadline) { Show-ServerLog $dir; throw 'no admin setup code in the server log' }
        Start-Sleep -Milliseconds 250
        $log = (Get-Content (Join-Path $dir 'server.out'), (Join-Path $dir 'server.err') -Raw -ErrorAction SilentlyContinue) -join "`n"
        $plain = $log -replace "`e\[[0-9;]*m", ''
        if ($plain -match 'Admin setup code: ([A-Z0-9]{4}-[A-Z0-9]{4})') { $code = $Matches[1] }
    }
    Write-Host 'server is up'

    $env:CHUMLY_SETUP_CODE = $code
    try {
        Invoke-Checked 'end_to_end, screen_stress' {
            cargo test -p client --locked -- --ignored end_to_end screen_stress --test-threads=1 --nocapture
        }
    } catch {
        Show-ServerLog $dir
        throw
    } finally {
        Remove-Item Env:CHUMLY_SETUP_CODE
    }
} finally {
    if (-not $server.HasExited) { Stop-Process -Id $server.Id -Force; $server.WaitForExit() }
}

$env:CHUMLY_SERVER_BIN = $bin
$env:CHUMLY_SERVER_CONFIG = $config
try {
    Invoke-Checked 'reconnects_after_server_restart' {
        cargo test -p client --locked -- --ignored reconnects_after_server_restart --nocapture
    }
} finally {
    Remove-Item Env:CHUMLY_SERVER_BIN, Env:CHUMLY_SERVER_CONFIG
}

Remove-Item -Recurse -Force $dir
Write-Host 'e2e: all passed'
