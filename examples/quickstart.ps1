# AgentGit end-to-end demo for Windows: a human, two agents and a CI service share one stream.
# This is examples/quickstart.sh in PowerShell; both are run by CI and must behave the same.
#
#   .\examples\quickstart.ps1                       # uses ag.exe from PATH, or target\{release,debug}\ag.exe
#   $env:AG = 'C:\path\to\ag.exe'; .\examples\quickstart.ps1
#
# Works in Windows PowerShell 5.1 and PowerShell 7. Every step asserts the exit code it expects, so CI runs
# this script as a test: the README transcript cannot drift away from what the tool really does.
$ErrorActionPreference = 'Stop'

$Here = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Ag = $env:AG
if (-not $Ag) {
    if (Get-Command ag -ErrorAction SilentlyContinue) { $Ag = (Get-Command ag).Source }
    elseif (Test-Path (Join-Path $Here 'target\release\ag.exe')) { $Ag = Join-Path $Here 'target\release\ag.exe' }
    elseif (Test-Path (Join-Path $Here 'target\debug\ag.exe')) { $Ag = Join-Path $Here 'target\debug\ag.exe' }
    else { Write-Error "ag not found: run 'cargo build --release' first"; exit 1 }
}

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("ag-quickstart-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Work | Out-Null
$Repo = Join-Path $Work 'webapp'
New-Item -ItemType Directory -Path $Repo | Out-Null

function Section($text) { Write-Host ""; Write-Host "== $text" -ForegroundColor Cyan }

# Files are written as UTF-8 without a byte-order mark and with LF line endings: the policy file is JSON.
function Put([string]$Path, [string]$Text) {
    $dir = Split-Path -Parent $Path
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
    [System.IO.File]::WriteAllText($Path, ($Text -replace "`r`n", "`n"), (New-Object System.Text.UTF8Encoding($false)))
}

# Run ag as someone, returning stdout lines. Throws if the exit code is not $Want (default 0).
function Get-Ag([string]$Who, [string[]]$AgArgs, [int]$Want = 0) {
    $env:AG_IDENTITY = $Who
    try {
        $out = & $Ag @AgArgs
        $got = $LASTEXITCODE
    }
    finally { Remove-Item Env:\AG_IDENTITY -ErrorAction SilentlyContinue }
    if ($got -ne $Want) { throw "ag $($AgArgs -join ' '): expected exit code $Want, got $got" }
    return $out
}

# Echo the command, run it, assert the exit code.
function Invoke-Ag([string]$Who, [int]$Want, [string[]]$AgArgs) {
    Write-Host ('$ ag ' + ($AgArgs -join ' ') + "   # as $Who") -ForegroundColor DarkGray
    $out = Get-Ag $Who $AgArgs $Want
    if ($out) { $out | ForEach-Object { Write-Host $_ } }
}

# Run quietly and parse the JSON on stdout.
function Get-AgJson([string]$Who, [string[]]$AgArgs) {
    $text = (Get-Ag $Who $AgArgs) -join "`n"
    return $text | ConvertFrom-Json
}

try {
    Set-Location $Repo

    Section '1. A repository, and its first commit'
    Invoke-Ag 'user:alice' 0 'init', '--namespace', 'acme/webapp'
    Put (Join-Path $Repo 'src\main.rs') @'
fn greeting() -> &'static str {
    "hello"
}

fn main() {
    println!("{}", greeting());
}
'@
    Put (Join-Path $Repo 'docs\README.md') "# webapp`n`nA small demo.`n"
    Invoke-Ag 'user:alice' 0 'snapshot', '-m', 'Initial import'
    $P0 = (Get-AgJson 'user:alice' '--json', 'propose', '-t', 'Initial import').id
    Invoke-Ag 'user:alice' 0 'integrate', $P0

    Section '2. Policy: CI must pass on the exact tree; agents leave provenance; humans approve risky changes'
    $KEY = (Get-AgJson 'user:alice' '--json', 'validate', 'keygen', 'ci').public_key
    Put (Join-Path $Work 'policy.json') @"
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 0, "min_for_agents": 1 },
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 5 },
  "require_provenance_for_agents": true
}
"@
    Invoke-Ag 'user:alice' 0 'policy', 'set', (Join-Path $Work 'policy.json')
    # The CI service validates the exact candidate tree of a proposal. `cmd` is what every Windows machine has.
    $Check = '(if exist docs\README.md if exist src\main.rs exit /b 0) & exit /b 1'
    function Invoke-Ci([string]$Proposal) {
        Invoke-Ag 'service:ci' 0 'validate', 'run', $Proposal, '--check', 'tests', '--key', 'ci', '--', 'cmd', '/c', $Check
    }

    Section '3. Two agents get identities and start runs'
    Invoke-Ag 'user:alice' 0 'agent', 'register', 'docs-bot'
    Invoke-Ag 'user:alice' 0 'agent', 'register', 'refactor-bot'
    $j = Get-AgJson 'user:alice' '--json', 'agent', 'start', 'docs-bot'
    $DOCS = $j.identity
    $j = Get-AgJson 'user:alice' '--json', 'agent', 'start', 'refactor-bot'
    $REFACTOR = $j.identity
    $REFACTOR_RUN = $j.run_id
    Write-Host "docs-bot acts as     $DOCS"
    Write-Host "refactor-bot acts as $REFACTOR"
    Invoke-Ag $DOCS 0 'workspace', 'create', 'docs', '--dir', (Join-Path $Work 'ws-docs')
    Invoke-Ag $REFACTOR 0 'workspace', 'create', 'refactor', '--dir', (Join-Path $Work 'ws-refactor')

    Section '4. docs-bot edits documentation: inside its autonomy envelope, no human needed'
    Put (Join-Path $Work 'ws-docs\docs\README.md') "# webapp`n`nA small demo.`n`n## Usage`n`nRun it.`n"
    Invoke-Ag $DOCS 0 'snapshot', '-m', 'Document usage', '--workspace', 'docs'
    $PD = (Get-AgJson $DOCS '--json', 'propose', '-t', 'Document usage', '@docs').id
    Invoke-Ag $DOCS 0 'provenance', 'attach', '@docs', '--agent', 'docs-bot', '--model', 'demo-model', '--rationale', 'usage section was missing'
    Invoke-Ci $PD
    Invoke-Ag $DOCS 0 'integrate', $PD

    Section '5. refactor-bot changes code: blocked until CI, provenance, a human and its lease allow it'
    Invoke-Ag $REFACTOR 0 'lease', 'acquire', 'src/**', '--exclusive', '--ttl', '30m'
    $TOKEN = (Get-AgJson $REFACTOR '--json', 'lease', 'list')[0].token
    Put (Join-Path $Work 'ws-refactor\src\main.rs') @'
fn greeting() -> &'static str {
    "hello, world"
}

fn main() {
    println!("{}", greeting());
}
'@
    Invoke-Ag $REFACTOR 0 'snapshot', '-m', 'Friendlier greeting', '--workspace', 'refactor'
    $PR = (Get-AgJson $REFACTOR '--json', 'propose', '-t', 'Friendlier greeting', '@refactor').id
    Invoke-Ag $REFACTOR 4 'integrate', $PR           # agents may not integrate code on their own
    Invoke-Ag 'user:alice' 3 'integrate', $PR        # blocked: no CI, no provenance, no approval, no token
    Invoke-Ci $PR
    Invoke-Ag $REFACTOR 0 'provenance', 'attach', '@refactor', '--agent', 'refactor-bot', '--model', 'demo-model', '--check', 'cargo test'
    Invoke-Ag 'user:bob' 0 'proposal', 'review', $PR, '--approve', '-m', 'reviewed, looks good'
    Invoke-Ag 'user:alice' 0 'integrate', $PR, '--fencing-token', $TOKEN
    Invoke-Ag $REFACTOR 0 'agent', 'finish', $REFACTOR_RUN   # finishing a run releases its leases

    Section '6. A concurrent edit conflicts; the stream is never touched; the conflict is resolved and lands'
    Invoke-Ag 'user:bob' 0 'workspace', 'create', 'bob', '--dir', (Join-Path $Work 'ws-bob')   # starts from the *new* head
    Put (Join-Path $Work 'ws-bob\src\main.rs') @'
fn greeting() -> &'static str {
    "hello, bob"
}

fn main() {
    println!("{}", greeting());
}
'@
    Invoke-Ag 'user:bob' 0 'snapshot', '-m', 'Greet bob', '--workspace', 'bob'
    $PB = (Get-AgJson 'user:bob' '--json', 'propose', '-t', 'Greet bob', '@bob').id
    # meanwhile alice syncs to the new head and lands another edit to the same line
    Invoke-Ag 'user:alice' 0 'sync'
    Put (Join-Path $Repo 'src\main.rs') @'
fn greeting() -> &'static str {
    "hello, alice"
}

fn main() {
    println!("{}", greeting());
}
'@
    Invoke-Ag 'user:alice' 0 'snapshot', '-m', 'Greet alice'
    $PA = (Get-AgJson 'user:alice' '--json', 'propose', '-t', 'Greet alice').id
    Invoke-Ci $PA
    Invoke-Ag 'user:alice' 0 'integrate', $PA
    Invoke-Ag 'user:bob' 4 'preview', $PB                    # conflict: reported before anything happens
    Invoke-Ag 'user:bob' 4 'sync', '--workspace', 'bob'      # conflict markers are written for bob to resolve
    Put (Join-Path $Work 'ws-bob\src\main.rs') @'
fn greeting() -> &'static str {
    "hello, alice and bob"
}

fn main() {
    println!("{}", greeting());
}
'@
    Invoke-Ag 'user:bob' 0 'snapshot', '-m', 'Greet both', '--workspace', 'bob'
    Invoke-Ag 'user:bob' 0 'proposal', 'update', $PB, '@bob'
    Invoke-Ci $PB
    Invoke-Ag 'user:alice' 0 'integrate', $PB

    Section '7. Everything is auditable and verifiable'
    Invoke-Ag 'user:alice' 0 'log', 'main', '-n', '8'
    Invoke-Ag 'user:alice' 0 'trace', $REFACTOR_RUN, '-n', '12'
    Invoke-Ag 'user:alice' 0 'verify'
    Invoke-Ag 'user:alice' 0 'audit'
    Write-Host ""
    Write-Host "Demo finished: all steps behaved as expected."
}
finally {
    Set-Location $Here
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
