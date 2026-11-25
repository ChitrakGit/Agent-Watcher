[CmdletBinding()]
param([switch]$Remove,[string]$Settings=(Join-Path $env:USERPROFILE '.claude\settings.json'),
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

# POSIX shells used by Claude Code on Windows accept a forward-slash executable path.
$native=$Executable.Replace('\','/')
if ($native.Contains("'")){throw 'Hook executable path contains a single quote'}
$command="'$native' claude-hook"
$config=[ordered]@{}
if(Test-Path -LiteralPath $Settings){
    $raw=Get-Content -Raw -LiteralPath $Settings
    if($raw -and $raw.Trim()){$config=ConvertTo-OrderedMap ($raw | ConvertFrom-Json)}
}
if (-not $config.Contains('hooks') -or $null -eq $config['hooks']){$config.hooks=[ordered]@{}}
foreach ($name in 'PreToolUse','PostToolUse','PostToolUseFailure','PermissionDenied'){
    $keep=@()
    foreach ($group in @($config.hooks[$name])){
        if ($null -eq $group){continue}
        if($group.matcher -ceq 'Bash|PowerShell'){
            $rest=@($group.hooks | Where-Object { $_.type -ne 'command' -or $_.command -cne $command })
            if($rest.Count -ne @($group.hooks).Count){
                if($rest.Count){$group.hooks=$rest;$keep+=,$group}
                continue
            }
        }
        $keep+=,$group
    }
    if(-not $Remove){$keep+=,[ordered]@{matcher='Bash|PowerShell';hooks=@([ordered]@{type='command';command=$command})}}
    if($keep.Count){$config.hooks[$name]=$keep}else{$config.hooks.Remove($name)}
}
$parent=Split-Path -Parent ([IO.Path]::GetFullPath($Settings))
New-Item -ItemType Directory -Force -Path $parent | Out-Null
$full=[IO.Path]::GetFullPath($Settings)
$tmp=Join-Path $parent ([guid]::NewGuid().ToString()+'.tmp')
try {
    [IO.File]::WriteAllText($tmp,($config | ConvertTo-Json -Depth 100))
    if([IO.File]::Exists($full)){[IO.File]::Replace($tmp,$full,[NullString]::Value)}else{[IO.File]::Move($tmp,$full)}
} finally {if(Test-Path -LiteralPath $tmp){Remove-Item -LiteralPath $tmp}}
