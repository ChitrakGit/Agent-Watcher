[CmdletBinding()]
param([switch]$Remove,[string]$HooksPath=(Join-Path $env:USERPROFILE '.gemini\config\hooks.json'),
      [string]$Executable=(Join-Path $env:ProgramFiles 'AgentWatcher\agent-watcher.exe'))
$ErrorActionPreference='Stop'
if (-not $Remove -and -not (Test-Path -LiteralPath $Executable -PathType Leaf)){throw 'Agent Watcher executable not found'}

function ConvertTo-OrderedMap($InputObject) {
    if ($null -eq $InputObject) { return $null }
    if ($InputObject -is [System.Collections.IDictionary]) {
        $map = [ordered]@{}
        foreach ($k in $InputObject.Keys) { $map[$k] = ConvertTo-OrderedMap $InputObject[$k] }
        return $map
    }
    if ($InputObject -is [System.Management.Automation.PSCustomObject]) {
        $map = [ordered]@{}
        foreach ($p in $InputObject.PSObject.Properties) { $map[$p.Name] = ConvertTo-OrderedMap $p.Value }
        return $map
    }
    if ($InputObject -is [System.Collections.IEnumerable] -and -not ($InputObject -is [string])) {
        $list = New-Object System.Collections.ArrayList
        foreach ($item in $InputObject) { [void]$list.Add((ConvertTo-OrderedMap $item)) }
        return ,$list.ToArray()
    }
    return $InputObject
}

$command="`"$Executable`" antigravity-hook"
$config=[ordered]@{}
if(Test-Path -LiteralPath $HooksPath){
    $raw=Get-Content -Raw -LiteralPath $HooksPath
    if($raw -and $raw.Trim()){$config=ConvertTo-OrderedMap ($raw | ConvertFrom-Json)}
}

if($Remove){
    $config.Remove('agent-watcher')
}else{
    $config['agent-watcher']=[ordered]@{
        PostToolUse=@(
            [ordered]@{
                matcher='run_command'
                hooks=@([ordered]@{type='command';command=$command})
            }
        )
    }
}

$full=[IO.Path]::GetFullPath($HooksPath)
$parent=Split-Path -Parent $full
New-Item -ItemType Directory -Force -Path $parent | Out-Null
$tmp=Join-Path $parent ([guid]::NewGuid().ToString()+'.tmp')
try {
    [IO.File]::WriteAllText($tmp,($config | ConvertTo-Json -Depth 100))
    if([IO.File]::Exists($full)){[IO.File]::Replace($tmp,$full,[NullString]::Value)}else{[IO.File]::Move($tmp,$full)}
} finally {if(Test-Path -LiteralPath $tmp){Remove-Item -LiteralPath $tmp}}
