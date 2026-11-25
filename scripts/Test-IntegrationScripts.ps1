#Requires -Version 7.2
[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
$fixture=Join-Path ([IO.Path]::GetTempPath()) ('watcher-hooks-'+[guid]::NewGuid())
[void][IO.Directory]::CreateDirectory($fixture)
$settings=Join-Path $fixture 'settings.json'
$fake=Join-Path $fixture 'agent-watcher.exe'
function Assert([bool]$Condition,[string]$Message){if(-not $Condition){throw $Message}}
try {
    $schema=Join-Path $repo 'schemas/event-v2.schema.json'
    $events=Get-Content -Raw -LiteralPath (Join-Path $repo 'tests/fixtures/chunk-v2.json') | ConvertFrom-Json -AsHashtable
    foreach ($e in $events) {
        Assert (Test-Json -Json ($e|ConvertTo-Json -Depth 8) -SchemaFile $schema) 'Invalid v2 fixture'
    }
    $invalid=$events[1].Clone()
    $invalid.Remove('sequence')
    $valid=Test-Json -Json ($invalid|ConvertTo-Json -Depth 8) -SchemaFile $schema -ErrorAction SilentlyContinue
    Assert (-not $valid) 'Schema accepted output without a sequence'
    foreach($script in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1'){
        $tokens=$null;$errors=$null
        [void][Management.Automation.Language.Parser]::ParseFile($script.FullName,[ref]$tokens,[ref]$errors)
        Assert ($errors.Count -eq 0) "Parse failure in $($script.Name)"
    }
    [IO.File]::WriteAllText($fake,'fixture path only')
    $initial=@{example='keep';hooks=@{PreToolUse=@(@{matcher='Bash|PowerShell';hooks=@(@{type='command';command='unrelated-tool'})})}}
    [IO.File]::WriteAllText($settings,($initial|ConvertTo-Json -Depth 8))
    & (Join-Path $PSScriptRoot 'ClaudeHooks.ps1') -Settings $settings -Executable $fake
    $once=Get-Content -Raw -LiteralPath $settings
    & (Join-Path $PSScriptRoot 'ClaudeHooks.ps1') -Settings $settings -Executable $fake
    $twice=Get-Content -Raw -LiteralPath $settings
    Assert ($once -ceq $twice) 'Repeated hook installation changed the config'
    $installed=$twice|ConvertFrom-Json -AsHashtable
    Assert ($installed.example -eq 'keep') 'Unrelated setting changed'
    Assert (@($installed.hooks.PreToolUse).Count -eq 2) 'Existing hook was lost or adapter was duplicated'
    Assert ($installed.hooks.Count -eq 4) 'Expected four configured hook events'
    & (Join-Path $PSScriptRoot 'ClaudeHooks.ps1') -Settings $settings -Executable $fake -Remove
    $removed=Get-Content -Raw -LiteralPath $settings|ConvertFrom-Json -AsHashtable
    Assert ($removed.hooks.Count -eq 1) 'Removal left extra event handlers'
    Assert ($removed.hooks.PreToolUse[0].hooks[0].command -eq 'unrelated-tool') 'Unrelated hook changed during removal'
    Write-Output 'PASS: script syntax, JSON Schema acceptance/rejection, hook installation, idempotence, and selective removal.'
} finally {
    foreach($p in @($settings,$fake)){if([IO.File]::Exists($p)){[IO.File]::Delete($p)}}
    [IO.Directory]::Delete($fixture,$false)
}
