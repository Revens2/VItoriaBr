# Recommended frame-rate environment for `victory run --console series-s` (see docs/performance.md).
# Usage:  .\scripts\series-s-preset.ps1 <victory.exe> <path-to-title-executable> [extra args of `victory run`]
# Dot-source it to only set the variables:  . .\scripts\series-s-preset.ps1
param(
    [string]$Victory,
    [string]$Title,
    [Parameter(ValueFromRemainingArguments = $true)] [string[]]$Extra
)

$preset = [ordered]@{
    VICTORY_RELAY            = "2"
    VICTORY_QUEUE2           = "1"
    VICTORY_ASYNC_SUBMIT     = "1"
    VICTORY_INDIRECT_WINDOW  = "1"
    VICTORY_BACK_OWN_VIEW    = "1"
    VICTORY_LATE_WAIT        = "3"
    VICTORY_MERGE_ON_CARD    = "1"
    VICTORY_PREP_PAR_COPY    = "1"
    VICTORY_PREP_PAR_HEAP    = "1"
    VICTORY_JOIN_PARTS       = "1"
}
foreach ($k in $preset.Keys) { Set-Item -Path "Env:$k" -Value $preset[$k] }

if ($Victory -and $Title) {
    & $Victory run --execute $Title --console series-s @Extra
}
