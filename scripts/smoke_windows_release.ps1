# Build a Windows archive from existing binaries, install it in isolation, then exercise first use.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [Parameter(Mandatory=$true)][string]$BinDir,
    [string]$Out = 'dist'
)
$ErrorActionPreference = 'Stop'
python scripts/package_release.py archive --version $Version --target x86_64-pc-windows-msvc --bin-dir $BinDir --out $Out
if ($LASTEXITCODE) { throw 'Windows archive packaging failed.' }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.ps1') -Destination (Join-Path $Out 'install.ps1')
$zip = Join-Path $Out "agentgit-$Version-x86_64-pc-windows-msvc.zip"
python scripts/package_release.py sums $zip | Set-Content -Encoding utf8 (Join-Path $Out 'SHA256SUMS')
if ($LASTEXITCODE) { throw 'Windows archive checksum generation failed.' }
$prefix = Join-Path ([IO.Path]::GetTempPath()) ('agentgit-windows-smoke-' + [guid]::NewGuid().ToString('N'))
try {
    & "$PSScriptRoot/install.ps1" -Version $Version -BaseUrl ([IO.Path]::GetFullPath($Out)) -Prefix $prefix -NoAttestation
    if ($LASTEXITCODE) { throw 'Windows isolated installation failed.' }
    foreach ($binary in @('ag.exe', 'ag-server.exe', 'ag-mcp.exe')) {
        & (Join-Path $prefix "bin/$binary") --version
        if ($LASTEXITCODE) { throw "$binary did not run from the installed archive." }
    }
    python scripts/onboarding_smoke.py --ag (Join-Path $prefix 'bin/ag.exe') --server (Join-Path $prefix 'bin/ag-server.exe')
    if ($LASTEXITCODE) { throw 'Installed Windows first-use workflow failed.' }
}
finally { Remove-Item -LiteralPath $prefix -Recurse -Force -ErrorAction SilentlyContinue }
