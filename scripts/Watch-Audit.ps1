[CmdletBinding()]
param([Parameter(Mandatory)][string]$AgentExecutable,
      [Parameter(Mandatory)][string]$AgentName,
      [string]$Watcher=(Join-Path $env:ProgramFiles 'AgentWatcher\agent-watcher.exe'),
      [string]$OwnerSid=([Security.Principal.WindowsIdentity]::GetCurrent().User.Value),
      [switch]$Once)
$ErrorActionPreference='Stop'
$agentPath=[IO.Path]::GetFullPath($AgentExecutable)
$state=Join-Path $env:LOCALAPPDATA 'AgentWatcher\audit'
New-Item -ItemType Directory -Force -Path $state | Out-Null
$sha=[Security.Cryptography.SHA256]::Create()
try {$tag=[BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($OwnerSid+'|'+$agentPath))).Replace('-','')} finally {$sha.Dispose()}
$bookmark=Join-Path $state ($tag+'.txt')
function Save-Record([long]$Value){
    $temp=$bookmark+'.tmp'
    [IO.File]::WriteAllText($temp,[string]$Value)
    if([IO.File]::Exists($bookmark)){[IO.File]::Replace($temp,$bookmark,$null)}else{[IO.File]::Move($temp,$bookmark)}
}
$last=if(Test-Path -LiteralPath $bookmark){[long](Get-Content -Raw -LiteralPath $bookmark)}else{
    $latest=Get-WinEvent -LogName Security -MaxEvents 1
    Save-Record $latest.RecordId
    [long]$latest.RecordId
}
$OutputEncoding=[Text.UTF8Encoding]::new($false)
do {
    $latest=Get-WinEvent -LogName Security -MaxEvents 1
    if ($latest.RecordId -lt $last){Write-Warning 'Security log reset. Resuming at the oldest available record.';$last=0}
    $xpath="*[System[(EventID=4688) and (EventRecordID > $last)]]"
    try {$events=@(Get-WinEvent -LogName Security -FilterXPath $xpath -Oldest -MaxEvents 512 -ErrorAction Stop)}
    catch {if($_.FullyQualifiedErrorId -like 'NoMatchingEventsFound*'){$events=@()}else{throw}}
    foreach($record in $events){
        [xml]$xml=$record.ToXml()
        $fields=@{}
        foreach($f in $xml.Event.EventData.Data){$fields[[string]$f.Name]=[string]$f.'#text'}
        $identity=$fields.TargetUserSid
        if(-not $identity -or $identity -eq 'S-1-0-0'){$identity=$fields.SubjectUserSid}
        if ($identity -eq $OwnerSid -and $fields.ParentProcessName -ieq $agentPath){
            $cmd=if($fields.CommandLine){$fields.CommandLine}else{$fields.NewProcessName}
            $utc=$record.TimeCreated.ToUniversalTime().ToString('o')
            $payload=@{key=($record.MachineName+'|'+$record.RecordId+'|'+$utc);app=$AgentName;command=$cmd;time=$utc;pid=[Convert]::ToUInt32($fields.NewProcessId,16)}
            ($payload | ConvertTo-Json -Compress) | & $Watcher audit-event
            if($LASTEXITCODE){throw 'Queue rejected observation. Bookmark retained for retry.'}
        }
        $last=$record.RecordId
        Save-Record $last
    }
    if(-not $Once -and $events.Count -lt 512){Start-Sleep -Seconds 3}
}while(-not $Once)
