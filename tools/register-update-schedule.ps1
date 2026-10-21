<#
.SYNOPSIS
    Register (or remove) the Playground update fill with
    Windows Task Scheduler.

.DESCRIPTION
    The update fill (update.exe) is Playground's scheduled
    network work: it refreshes the personal update database
    (catalog-home.json) so the app can answer "what's new"
    locally, in milliseconds, at launch. John's cadence:
    about three runs a day.

    This script registers one scheduled task,
    "Playground Update Fill", with one daily trigger per
    time in -Times. Re-running the script simply
    re-registers the task (-Force): no duplicates, no
    leftovers -- the registration is idempotent.

    The task runs as the user who registers it, only when
    that user is logged on, and Task Scheduler will start
    it as soon as possible after a missed start (a machine
    that was asleep at 14:00 gets its fill when it wakes).

.PARAMETER UpdateExe
    Path to update.exe. Default: ..\update.exe relative to
    this script (the tools folder sits beside the binary
    in a release layout). Point it wherever the binary
    actually lives.

.PARAMETER Times
    Local times of day for the fill. THE DEFAULTS ARE
    06:00, 14:00 and 22:00 -- change them to whatever
    suits the household; they are just defaults.
    Example: -Times '07:30','19:30' for twice a day.

.PARAMETER Remove
    Unregister the task and exit.

.EXAMPLE
    .\register-update-schedule.ps1
    Registers the fill at the default three times.

.EXAMPLE
    .\register-update-schedule.ps1 -UpdateExe 'C:\Playground\update.exe' -Times '06:00','14:00','22:00'

.EXAMPLE
    .\register-update-schedule.ps1 -Remove
    Removes the scheduled task.
#>
[CmdletBinding()]
param(
    [string]$UpdateExe = (Join-Path $PSScriptRoot '..\update.exe'),
    [string[]]$Times = @('06:00', '14:00', '22:00'),
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'
$TaskName = 'Playground Update Fill'

if ($Remove) {
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "Removed scheduled task '$TaskName'."
    }
    else {
        Write-Host "Scheduled task '$TaskName' is not registered -- nothing to remove."
    }
    return
}

if (-not (Test-Path $UpdateExe)) {
    throw "update.exe not found at '$UpdateExe'. Pass -UpdateExe with the real path (the binary the release ships, beside fs-mod-manager.exe)."
}
$ExePath = (Resolve-Path $UpdateExe).Path
$ExeDir = Split-Path $ExePath

# The verb is explicit: 'update' is the fill. (Bare
# update.exe runs the fill too -- the scheduler passes the
# verb so the task's command line reads as what it does.)
$Action = New-ScheduledTaskAction -Execute $ExePath -Argument 'update' -WorkingDirectory $ExeDir

$Triggers = foreach ($t in $Times) {
    New-ScheduledTaskTrigger -Daily -At $t
}

# StartWhenAvailable: a missed run (machine asleep/off)
# fires as soon as the machine is back. MultipleInstances
# IgnoreNew: a still-running fill is never doubled up.
$Settings = New-ScheduledTaskSettingsSet `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit (New-TimeSpan -Hours 2)

$Principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited

$Description = 'Playground: refresh the personal update database (catalog-home.json) -- scan facts + ModHub catalog join + link polls. Runs update.exe (the isolated update program). Registered by tools\register-update-schedule.ps1.'

# -Force makes re-running this script a clean
# re-registration instead of a duplicate or an error.
Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Triggers `
    -Settings $Settings `
    -Principal $Principal `
    -Description $Description `
    -Force | Out-Null

Write-Host "Registered '$TaskName':"
Write-Host "  runs:  $ExePath update"
Write-Host "  times: $($Times -join ', ') daily (local)"
Write-Host "Remove it with: .\register-update-schedule.ps1 -Remove"
