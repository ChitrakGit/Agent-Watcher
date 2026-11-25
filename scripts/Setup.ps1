[CmdletBinding()]
param(
    [string]$BinaryDirectory = '',
    [Nullable[long]]$RetentionBytes = $null,
    [switch]$Rebuild,
    [switch]$SkipClaudeHooks,
    [switch]$SkipAntigravityHooks,
    [switch]$LaunchViewer,
    [switch]$Uninstall,
    [switch]$ElevatedTaskOnly,
    [string]$LogFile = ''
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $BinaryDirectory) {
    $BinaryDirectory = Join-Path $repoRoot 'target\release'
}
$installDir = Join-Path $env:ProgramFiles 'AgentWatcher'
$installedExe = Join-Path $installDir 'agent-watcher.exe'
$installedView = Join-Path $installDir 'agent-watcher-view.exe'

function Test-IsAdmin {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Invoke-ElevatedTask {
    if ($Uninstall) {
        & (Join-Path $PSScriptRoot 'Uninstall.ps1')
        return
    }
    $runningViewer = Get-Process -Name 'agent-watcher-view' -ErrorAction SilentlyContinue
    if ($runningViewer) {
        $runningViewer | Stop-Process -Force
        Start-Sleep -Milliseconds 300
    }
    $installArgs = @{ BinaryDirectory = $BinaryDirectory }
    if ($null -ne $RetentionBytes) { $installArgs['RetentionBytes'] = $RetentionBytes }
    & (Join-Path $PSScriptRoot 'Install.ps1') @installArgs
}

if ($ElevatedTaskOnly) {
    try {
        $output = Invoke-ElevatedTask 2>&1 | Out-String
        if ($LogFile) { [IO.File]::WriteAllText($LogFile, $output) }
        exit 0
    } catch {
        $errText = ($_ | Out-String)
        if ($LogFile) { [IO.File]::WriteAllText($LogFile, $errText) }
        Write-Error $_
        exit 1
    }
}

if (-not $Uninstall) {
    $needBuild = $Rebuild -or
        -not (Test-Path -LiteralPath (Join-Path $BinaryDirectory 'agent-watcher.exe') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $BinaryDirectory 'agent-watcher-view.exe') -PathType Leaf)
    if ($needBuild) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            throw 'Release binaries were not found and cargo is not on PATH. Install Rust or pass -BinaryDirectory.'
        }
        Write-Output 'Building Agent Watcher release binaries...'
        Push-Location $repoRoot
        try {
            if (Test-Path -LiteralPath (Join-Path $repoRoot 'Cargo.lock')) {
                & cargo build --locked --release --bins
            } else {
                & cargo build --release --bins
            }
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
        } finally {
            Pop-Location
        }
    }
}

if (Test-IsAdmin) {
    Invoke-ElevatedTask
} else {
    Write-Output 'Requesting Administrator permission (UAC) to configure the AgentWatcher Windows service...'
    $tempLog = Join-Path ([IO.Path]::GetTempPath()) ("agent-watcher-setup-" + [guid]::NewGuid().ToString() + ".log")
    $argList = @(
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-File', ('"' + $PSCommandPath + '"'),
        '-ElevatedTaskOnly',
        '-BinaryDirectory', ('"' + $BinaryDirectory + '"'),
        '-LogFile', ('"' + $tempLog + '"')
    )
    if ($null -ne $RetentionBytes) { $argList += @('-RetentionBytes', [string]$RetentionBytes) }
    if ($Uninstall) { $argList += '-Uninstall' }

    $shellExe = (Get-Process -Id $PID).Path
    try {
        $proc = Start-Process -FilePath $shellExe -ArgumentList $argList -WorkingDirectory $repoRoot -Verb RunAs -Wait -PassThru
    } catch [System.ComponentModel.Win32Exception] {
        throw 'Administrator permission was canceled or denied. Setup cannot install or remove the Windows service without elevation.'
    }
    $elevatedOutput = if (Test-Path -LiteralPath $tempLog) {
        try { Get-Content -Raw -LiteralPath $tempLog } finally { Remove-Item -LiteralPath $tempLog -Force -ErrorAction SilentlyContinue }
    } else { '' }
    if ($elevatedOutput) { Write-Output $elevatedOutput.TrimEnd() }
    if ($proc.ExitCode -ne 0) {
        throw "Elevated setup step failed (exit code $($proc.ExitCode))."
    }
}

if ($Uninstall) {
    if (-not $SkipClaudeHooks -and (Test-Path -LiteralPath (Join-Path $env:USERPROFILE '.claude\settings.json'))) {
        & (Join-Path $PSScriptRoot 'ClaudeHooks.ps1') -Remove -Executable $installedExe
        Write-Output 'Removed Agent Watcher hooks from Claude Code settings.'
    }
    if (-not $SkipAntigravityHooks -and (Test-Path -LiteralPath (Join-Path $env:USERPROFILE '.gemini\config\hooks.json'))) {
        & (Join-Path $PSScriptRoot 'AntigravityHooks.ps1') -Remove -Executable $installedExe
        Write-Output 'Removed Agent Watcher hooks from Antigravity settings.'
    }
    Write-Output 'Agent Watcher uninstall complete.'
    exit 0
}

if (-not $SkipClaudeHooks) {
    & (Join-Path $PSScriptRoot 'ClaudeHooks.ps1') -Executable $installedExe
    Write-Output "Configured Claude Code hooks in $(Join-Path $env:USERPROFILE '.claude\settings.json')."
}

if (-not $SkipAntigravityHooks) {
    & (Join-Path $PSScriptRoot 'AntigravityHooks.ps1') -Executable $installedExe
    Write-Output "Configured Antigravity hooks in $(Join-Path $env:USERPROFILE '.gemini\config\hooks.json')."
}

$svc = Get-Service AgentWatcher -ErrorAction Stop
if ($svc.Status -ne 'Running') {
    throw "AgentWatcher service is not running (status: $($svc.Status))."
}
$health = & $installedExe status
Write-Output "Service status: $($svc.Status)"
Write-Output "Collector health: $health"
Write-Output 'System shell audit: AgentWatcherAudit scheduled task (Event 4688) enabled.'
Write-Output ''
Write-Output 'Setup complete.'
Write-Output 'If VS Code or Antigravity is open, reload/restart it so it picks up the new hooks.'
Write-Output "Open the viewer anytime from the Start Menu ('Agent Watcher') or by running: `"$installedView`""

if ($LaunchViewer) {
    Start-Process -FilePath $installedView
}
