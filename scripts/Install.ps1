#Requires -RunAsAdministrator
[CmdletBinding()]
param([string]$BinaryDirectory = (Join-Path $PSScriptRoot '..\target\release'),
      [Nullable[long]]$RetentionBytes = $null)
$ErrorActionPreference = 'Stop'
$id = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$base = Join-Path $env:ProgramData 'AgentWatcher'
$root = Join-Path $base $id
$install = Join-Path $env:ProgramFiles 'AgentWatcher'
$inbox = Join-Path $root 'inbox'
$exe = Join-Path $install 'agent-watcher.exe'
$view = Join-Path $install 'agent-watcher-view.exe'
if ($null -ne $RetentionBytes -and $RetentionBytes -lt 0) { throw 'RetentionBytes must be nonnegative.' }
$system = '*S-1-5-18'
$admins = '*S-1-5-32-544'
$service = '*S-1-5-19'
$owner = '*' + $id

function Assert-TrustedDirectory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Unexpected directory type at $Path"
    }
    $acl = Get-Acl -LiteralPath $Path
    $principal = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($principal -notin @('S-1-5-32-544', 'S-1-5-18', $id)) { throw "Unexpected directory owner at $Path" }
}

function Protect-Directory([string]$Path, [hashtable]$Rules) {
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    foreach ($name in $Rules.Keys) {
        $sid = [Security.Principal.SecurityIdentifier]::new($name.TrimStart('*'))
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid,$Rules[$name],'ContainerInherit,ObjectInherit','None','Allow')
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

foreach ($file in 'agent-watcher.exe','agent-watcher-view.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $BinaryDirectory $file) -PathType Leaf)) { throw "Build the release binaries first. Missing $file" }
}
$existing = Get-CimInstance Win32_Service -Filter "Name='AgentWatcher'"
if ($existing -and $existing.PathName -ne ('"{0}" --home "{1}" service' -f $exe,$root)) {
    throw 'An AgentWatcher service is already registered with a different identity or log root.'
}
if ($existing -and $existing.State -ne 'Stopped') { Stop-Service AgentWatcher -ErrorAction Stop }
$auditTaskName = 'AgentWatcherAudit'
if (Get-ScheduledTask -TaskName $auditTaskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $auditTaskName -ErrorAction SilentlyContinue
}
Assert-TrustedDirectory $install
Assert-TrustedDirectory $base
Protect-Directory $base @{$system='FullControl';$admins='FullControl';$service='ReadAndExecute';$owner='ReadAndExecute'}
Assert-TrustedDirectory $root
Protect-Directory $root @{$system='FullControl';$admins='FullControl';$service='Modify';$owner='ReadAndExecute'}
Assert-TrustedDirectory $inbox
Protect-Directory $inbox @{$system='FullControl';$admins='FullControl';$service='Modify';$owner='Modify'}
# The parent is read-only to the enrolled user. Keep the inbox itself fixed in place.
$acl=Get-Acl -LiteralPath $inbox
$sid=[Security.Principal.SecurityIdentifier]::new($id)
$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($sid,'Delete','None','None','Deny'))
Set-Acl -LiteralPath $inbox -AclObject $acl
Copy-Item -LiteralPath (Join-Path $BinaryDirectory 'agent-watcher.exe') -Destination $exe -Force
Copy-Item -LiteralPath (Join-Path $BinaryDirectory 'agent-watcher-view.exe') -Destination $view -Force
$line = '"{0}" --home "{1}" service' -f $exe,$root
if (-not $existing) {
    $result=Invoke-CimMethod -ClassName Win32_Service -MethodName Create -Arguments @{
        Name='AgentWatcher'; DisplayName='Agent Watcher'; PathName=$line; StartMode='Automatic';
        StartName='NT AUTHORITY\LocalService'; ServiceType=[byte]16; ErrorControl=[byte]1
    }
    if ($result.ReturnValue -ne 0) { throw "Service registration failed ($($result.ReturnValue))" }
}
& sc.exe failure AgentWatcher reset= 86400 actions= restart/5000/restart/15000/restart/60000
if ($LASTEXITCODE) {throw 'Could not configure service recovery'}
if ($null -ne $RetentionBytes) {[IO.File]::WriteAllText((Join-Path $root 'retention-bytes.txt'),[string]$RetentionBytes)}
Start-Service AgentWatcher
$registry='HKLM:\SOFTWARE\AgentWatcher'
New-Item -Path $registry -Force | Out-Null
New-ItemProperty -Path $registry -Name DataRoot -Value $root -PropertyType String -Force | Out-Null
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Agent Watcher.lnk'))
$link.TargetPath=$view
$link.Save()

# Enable OS-level process creation + command line auditing (Event 4688) for background shell capture.
& auditpol.exe /set /subcategory:"{0CCE922B-69AE-11D9-BED3-505054503030}" /success:enable | Out-Null
if ($LASTEXITCODE) {
    & auditpol.exe /set /subcategory:"Process Creation" /success:enable | Out-Null
}
$auditPolicyReg = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\Audit'
New-Item -Path $auditPolicyReg -Force | Out-Null
New-ItemProperty -Path $auditPolicyReg -Name ProcessCreationIncludeCmdLine_Enabled -Value 1 -PropertyType DWord -Force | Out-Null

$taskArgs = '--home "{0}" audit-watch --owner-sid "{1}"' -f $root, $id
$taskAction = New-ScheduledTaskAction -Execute $exe -Argument $taskArgs
$taskTrigger = New-ScheduledTaskTrigger -AtStartup
$taskPrincipal = New-ScheduledTaskPrincipal -UserId 'NT AUTHORITY\SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$taskSettings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $auditTaskName -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Settings $taskSettings -Force | Out-Null
Start-ScheduledTask -TaskName $auditTaskName

Write-Output "Installed Agent Watcher. Logs: $root\logs"
