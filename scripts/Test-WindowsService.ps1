#Requires -RunAsAdministrator
[CmdletBinding()]
param([string]$BinaryDirectory=(Join-Path $PSScriptRoot '..\target\release'),
      [Parameter(Mandatory)][switch]$DisposableMachine)
$ErrorActionPreference='Stop'
if(-not $DisposableMachine -or (Get-Service AgentWatcher -ErrorAction SilentlyContinue)){
    throw 'This lifecycle test requires a disposable Windows machine without an existing AgentWatcher service.'
}
if(Test-Path -LiteralPath (Join-Path $env:ProgramFiles 'AgentWatcher')){throw 'Existing installation directory. Use a clean VM.'}
try{
    & (Join-Path $PSScriptRoot 'Install.ps1') -BinaryDirectory $BinaryDirectory
    $exe=Join-Path $env:ProgramFiles 'AgentWatcher\agent-watcher.exe'
    $label='service-fixture-'+[guid]::NewGuid().ToString()
    & $exe run --agent $label -- cmd.exe /d /c 'echo SERVICE_FIXTURE'
    if($LASTEXITCODE){throw 'Command wrapper failed'}
    $deadline=[datetime]::UtcNow.AddSeconds(45)
    $matched=$false
    do{
        $old=$ErrorActionPreference
        try{
            $ErrorActionPreference='Continue'
            $raw=& $exe search --agent $label --output SERVICE_FIXTURE --outcome succeeded 2>$null
            if($LASTEXITCODE -eq 0){
                $rows=@(ConvertFrom-Json -InputObject ($raw -join [Environment]::NewLine))
                $matched=$rows.Count -eq 1 -and $rows[0].app -eq $label
            }
        }finally{$ErrorActionPreference=$old}
        if(-not $matched){Start-Sleep -Milliseconds 250}
    }while(-not $matched -and [datetime]::UtcNow -lt $deadline)
    if(-not $matched){throw 'Background collector failed to return the expected command'}
    Stop-Service AgentWatcher
    $root=Get-ItemPropertyValue -Path 'HKLM:\SOFTWARE\AgentWatcher' -Name DataRoot
    $files=Get-ChildItem -Path (Join-Path $root 'logs') -Filter '*.json'
    if(-not $files){throw 'Stopping the service did not publish a log'}
    foreach($f in $files){
        if($f.Length -gt 50000000){throw 'Oversized log'}
        $null=Get-Content -Raw -LiteralPath $f.FullName|ConvertFrom-Json
    }
    Start-Service AgentWatcher
    if((Get-Service AgentWatcher).Status -ne 'Running'){throw 'Service did not restart'}
    Write-Output 'PASS: installed, captured with viewer closed, sealed logs, and restarted.'
}finally{
    if(Get-Service AgentWatcher -ErrorAction SilentlyContinue){& (Join-Path $PSScriptRoot 'Uninstall.ps1')}
}
