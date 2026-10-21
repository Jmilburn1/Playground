# Scheduling the update fill

Playground's update checks are split in two, on purpose:

- **The app never crawls.** At launch it reads the personal
  update database (`catalog-home.json`, in
  `C:\Users\<you>\.config\playground\`) and answers
  "what's new for me" locally, in milliseconds.
- **The update program does the asking.** `update.exe` —
  the standalone binary that ships beside
  `playground.exe` — runs the fill: scan facts, the
  ModHub catalog join, and one polite poll per linked
  source (GitHub, KingMods, itch.io). It's built to run
  unattended, about **three times a day**, so the database
  the app reads is never more than a few hours old.

## Install the schedule

From a PowerShell prompt in this `tools` folder:

```powershell
.\register-update-schedule.ps1
```

That registers one Task Scheduler task, **Playground
Update Fill**, running `update.exe update` daily at
**06:00, 14:00 and 22:00** local time. If `update.exe`
isn't in the folder above this one, point the script at
it:

```powershell
.\register-update-schedule.ps1 -UpdateExe 'C:\path\to\update.exe'
```

The defaults are just defaults — pick your own times (or
fewer of them):

```powershell
.\register-update-schedule.ps1 -Times '07:30','19:30'
```

Notes:

- The script is **idempotent**: run it again after moving
  the binary or changing the times and it re-registers
  the same single task (`-Force`). No duplicates.
- The task runs **as you**, only when you're logged on,
  and fires as soon as possible after a missed start — a
  PC asleep at 14:00 gets its fill when it wakes.
- A fill that hits the **GIANTS tripwire** (ModHub
  answering HTTP 403/429) halts and exits nonzero without
  touching the database; Task Scheduler's history will
  show that run as failed, and the next scheduled run
  simply tries again.

## Remove the schedule

```powershell
.\register-update-schedule.ps1 -Remove
```

(Or delete the **Playground Update Fill** task in Task
Scheduler — same thing.)

## Run a fill by hand

Any time, from any prompt:

```powershell
update.exe            # bare = the fill
update.exe update     # the fill, explicitly
update.exe detect-modhub   # also available standalone
```

`update.exe help` lists everything it does. The old verb
name `db-fill` still works everywhere as a deprecated
alias of the fill.
