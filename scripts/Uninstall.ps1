#Requires -RunAsAdministrator
[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$install=Join-Path $env:ProgramFiles 'AgentWatcher'
$svc=Get-CimInstance Win32_Service -Filter "Name='AgentWatcher'"
$auditTask = Get-ScheduledTask -TaskName 'AgentWatcherAudit' -ErrorAction SilentlyContinue
if ($auditTask) {
    Stop-ScheduledTask -TaskName 'AgentWatcherAudit' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'AgentWatcherAudit' -Confirm:$false
}
if ($svc){
    if (-not $svc.PathName.StartsWith(('"'+(Join-Path $install 'agent-watcher.exe')+'" '),[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected service executable'}
    if ($svc.State -ne 'Stopped'){Stop-Service AgentWatcher}
    & sc.exe delete AgentWatcher
    if ($LASTEXITCODE){throw 'Service removal failed'}
}
foreach ($file in 'agent-watcher.exe','agent-watcher-view.exe'){
    $p=Join-Path $install $file
    if (Test-Path -LiteralPath $p){Remove-Item -LiteralPath $p}
}
$link=Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Agent Watcher.lnk'
if (Test-Path -LiteralPath $link){Remove-Item -LiteralPath $link}
Write-Output 'Uninstalled the service and binaries. The log files and their location pointer were preserved.'
