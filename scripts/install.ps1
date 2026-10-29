<#
.SYNOPSIS
Installs AgentGit's native Windows CLI, server and MCP client for the current user.
.DESCRIPTION
Downloads a published Windows ZIP and SHA256SUMS, verifies it, then installs only the
three expected executables. No administrator rights or permanent PATH edits are needed.
Windows archives are planned for the next release; beta.7 has no Windows ZIP.
.PARAMETER Version
Release version, without v. By default prefer the newest published stable release,
or a prerelease when no stable release has a compatible Windows archive.
.PARAMETER Prefix
Installation directory; defaults to LOCALAPPDATA\AgentGit. Binaries go in bin.
.PARAMETER BaseUrl
Release mirror URL or local directory, for offline installation. Requires Version.
#>
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$Prefix = '',
    [string]$BaseUrl = '',
    [switch]$DryRun,
    [switch]$RequireAttestation,
    [switch]$NoAttestation
)
$ErrorActionPreference = 'Stop'

function Find-WindowsRelease {
    param([object[]]$Releases)
    return @($Releases | Where-Object {
        $expected = 'agentgit-' + ($_.tag_name -replace '^v', '') + '-x86_64-pc-windows-msvc.zip'
        $_.draft -eq $false -and $_.tag_name -and @($_.assets | Where-Object { $_.name -ceq $expected }).Count
    })
}

function Get-LatestWindowsVersion {
    $firstPreview = $null
    $page = 1
    do {
        $url = "https://api.github.com/repos/Paffin/AgentGit/releases?per_page=100&page=$page"
        $batch = Invoke-RestMethod -Uri $url -Headers @{'User-Agent'='AgentGit installer'} -TimeoutSec 30
        $compatible = @(Find-WindowsRelease $batch)
        $stable = @($compatible | Where-Object { $_.prerelease -eq $false })
        if ($stable.Count) { return $stable[0].tag_name }
        if (-not $firstPreview -and $compatible.Count) { $firstPreview = $compatible[0] }
        $page++
    } while ($batch.Count -eq 100)
    if (-not $firstPreview) { throw 'No Windows archive published yet. See docs/operations/windows.md for source builds.' }
    Write-Host "Using published prerelease $($firstPreview.tag_name) (no compatible stable release found)."
    return $firstPreview.tag_name
}

function Resolve-Version {
    param([string]$Requested)
    if ($Requested -eq 'latest') {
        if ($BaseUrl) { throw '-BaseUrl requires an explicit -Version.' }
        $Requested = Get-LatestWindowsVersion
    }
    $clean = $Requested -replace '^v', ''
    if ($clean -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?$') { throw "Invalid release version: $Requested" }
    return $clean
}

function Get-ReleaseFile {
    param([string]$Base, [string]$Name, [string]$Destination)
    if ($Base -match '^https?://') {
        Invoke-WebRequest -UseBasicParsing -Uri ($Base.TrimEnd('/') + '/' + $Name) -OutFile $Destination -TimeoutSec 30
    }
    else {
        if ($Base.StartsWith('file://')) { $Base = ([Uri]$Base).LocalPath }
        Copy-Item -LiteralPath (Join-Path $Base $Name) -Destination $Destination
    }
}

function Assert-Checksum {
    param([string]$Archive, [string]$Sums, [string]$Name)
    $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($Name) + '$'
    $matches = @(Get-Content -LiteralPath $Sums | Where-Object { $_ -match $pattern })
    if ($matches.Count -ne 1) { throw "SHA256SUMS must contain exactly one checksum for $Name." }
    $expected = [regex]::Match($matches[0], $pattern).Groups[1].Value
    if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ne $expected) {
        throw 'Archive checksum mismatch. Nothing was installed.'
    }
    Write-Host 'Archive checksum verified.'
}

function Assert-Attestation {
    param([string]$Archive)
    # Windows PowerShell 5.1 converts native stderr into ErrorRecords. Inspect native exit
    # codes explicitly so an unauthenticated gh can fall back to checksum-only verification.
    $ErrorActionPreference = 'Continue'
    if ($NoAttestation) { return }
    $gh = Get-Command gh -ErrorAction SilentlyContinue
    if ($gh) {
        & $gh.Source auth status *> $null
        if ($LASTEXITCODE -eq 0) {
            & $gh.Source attestation verify $Archive --repo Paffin/AgentGit
            if ($LASTEXITCODE -ne 0) { throw 'Build-provenance verification failed. Nothing was installed.' }
            return
        }
    }
    if ($RequireAttestation) { throw '-RequireAttestation needs the GitHub CLI installed and logged in.' }
    Write-Host 'Note: checksum verified; build provenance needs an authenticated GitHub CLI.'
}

function Expand-Executables {
    param([string]$Archive, [string]$Top, [string]$Destination)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($binary in @('ag.exe', 'ag-server.exe', 'ag-mcp.exe')) {
            $entries = @($zip.Entries | Where-Object { $_.FullName -ceq "$Top/$binary" })
            if ($entries.Count -ne 1) { throw "Archive must contain exactly one $binary. Nothing was installed." }
            # Only exact known entries are extracted; arbitrary paths in the archive are never used.
            $inputStream = $entries[0].Open()
            try {
                $outputStream = [IO.File]::Create((Join-Path $Destination $binary))
                try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose() }
            }
            finally { $inputStream.Dispose() }
        }
    }
    finally { $zip.Dispose() }
}

function Quote-Path {
    param([string]$Path)
    return "'" + $Path.Replace("'", "''") + "'"
}

try {
    if ($RequireAttestation -and $NoAttestation) { throw 'Choose either -RequireAttestation or -NoAttestation.' }
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
    if ($architecture -and $architecture -ne 'AMD64') { throw 'Native archives currently support x86_64 Windows only.' }
    $resolved = Resolve-Version $Version
    if (-not $Prefix) {
        if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is not set; pass -Prefix with a directory you own.' }
        $Prefix = Join-Path $env:LOCALAPPDATA 'AgentGit'
    }
    $Prefix = [IO.Path]::GetFullPath($Prefix)
    $bin = Join-Path $Prefix 'bin'
    $top = "agentgit-$resolved-x86_64-pc-windows-msvc"
    $name = "$top.zip"
    if (-not $BaseUrl) { $BaseUrl = "https://github.com/Paffin/AgentGit/releases/download/v$resolved" }
    Write-Host "AgentGit $resolved for Windows x86_64"
    Write-Host "  from $BaseUrl/$name"
    Write-Host "  into $bin"
    if ($DryRun) { Write-Host 'Dry run: nothing was downloaded or installed.'; return }
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('agentgit-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temp | Out-Null
    try {
        $archive = Join-Path $temp $name
        Get-ReleaseFile $BaseUrl $name $archive
        $sums = Join-Path $temp 'SHA256SUMS'
        Get-ReleaseFile $BaseUrl 'SHA256SUMS' $sums
        Assert-Checksum $archive $sums $name
        Assert-Attestation $archive
        Expand-Executables $archive $top $temp
        New-Item -ItemType Directory -Path $bin -Force | Out-Null
        foreach ($binary in @('ag.exe', 'ag-server.exe', 'ag-mcp.exe')) {
            Copy-Item -LiteralPath (Join-Path $temp $binary) -Destination (Join-Path $bin $binary) -Force
        }
    }
    finally { Remove-Item -LiteralPath $temp -Recurse -Force }
    Write-Host 'Installed ag.exe, ag-server.exe and ag-mcp.exe.'
    Write-Host 'To use ag in this terminal:'
    Write-Host ('  $env:Path = ' + (Quote-Path $bin) + ' + [IO.Path]::PathSeparator + $env:Path')
    Write-Host 'Try the local demo in your browser:'
    Write-Host ('  & ' + (Quote-Path (Join-Path $bin 'ag-server.exe')) + ' dev --open')
}
catch {
    Write-Error $_ -ErrorAction Continue
    exit 1
}
