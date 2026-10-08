param([Parameter(Mandatory)][string]$Companion)

# GitHub's Windows runner is elevated. Native Codex correctly refuses to start
# a shared daemon with that token, so use a disposable standard account on the
# disposable CI VM. Never create an account on a developer's computer.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'Run test-tui-instances.py from a non-elevated terminal outside GitHub Actions.'
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    & python3 (Join-Path $PSScriptRoot 'test-tui-instances.py') --companion $Companion
    exit $LASTEXITCODE
}

$suffix = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$name = 'tui-' + $suffix
$root = Join-Path $env:SystemDrive ('t' + $suffix)
$taskName = 'AgentCompanion-TuiAcceptance-' + $suffix
$registered = $false
$created = $null
$result = 1
try {
    $secret = ConvertTo-SecureString ('aA1!' + [Guid]::NewGuid().ToString('N')) -AsPlainText -Force
    $created = New-LocalUser -Name $name -Password $secret -Description 'Disposable native TUI CI fixture'
    Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $created
    New-Item -ItemType Directory -Path $root | Out-Null
    $sid = '*' + $created.SID.Value
    & icacls.exe $root /inheritance:r /grant:r "${sid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' /Q | Out-Null
    if ($LASTEXITCODE) { throw 'Cannot prepare the disposable fixture directory.' }
    $companionPath = (Resolve-Path $Companion).Path
    # Only grant read/execute access to the test code and compiled test entries.
    foreach ($directory in @($PSScriptRoot, (Split-Path $companionPath))) {
        & icacls.exe $directory /grant "${sid}:(OI)(CI)RX" /T /Q | Out-Null
        if ($LASTEXITCODE) { throw 'Cannot grant fixture access to test binaries.' }
    }
    $launch = @{
        python = (Get-Command python3).Source
        script = Join-Path $PSScriptRoot 'test-tui-instances.py'
        companion = $companionPath
        fixture = Join-Path $root 'f'
        path = $env:Path
    }
    [IO.File]::WriteAllText((Join-Path $root 'launch.json'), ($launch | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    $bootstrap = Join-Path $root 'run.ps1'
    [IO.File]::WriteAllText($bootstrap, @'
$ErrorActionPreference = 'Stop'
$result = 1
$log = Join-Path $PSScriptRoot 'acceptance.log'
try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if ([Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Native daemon acceptance must use a standard user.'
    }
    $launch = Get-Content (Join-Path $PSScriptRoot 'launch.json') -Raw | ConvertFrom-Json
    $env:Path = $launch.path
    $env:PYTHONIOENCODING = 'utf-8'
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    $env:TEMP = Join-Path $PSScriptRoot 'tmp'
    $env:TMP = $env:TEMP
    New-Item -ItemType Directory -Path $env:TEMP | Out-Null
    & $launch.python $launch.script --companion $launch.companion --work-dir $launch.fixture *>&1 | Out-File -FilePath $log -Encoding utf8
    $result = $LASTEXITCODE
} catch {
    $_ | Out-File -FilePath $log -Encoding utf8 -Append
} finally {
    $pending = Join-Path $PSScriptRoot 'result.pending'
    [IO.File]::WriteAllText($pending, [string]$result)
    [IO.File]::Move($pending, (Join-Path $PSScriptRoot 'result'))
}
exit $result
'@, [Text.UTF8Encoding]::new($false))
    $shell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    # GitHub's outer runner job prevents native daemon breakaway even with a
    # different logon. Task Scheduler supplies an independent standard-user
    # host; neither production launch flags nor the runner's job are changed.
    $action = New-ScheduledTaskAction -Execute $shell -WorkingDirectory $root -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$bootstrap`""
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 25) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Register-ScheduledTask -TaskName $taskName -Action $action -Settings $settings -User "$env:COMPUTERNAME\$name" -Password ([Net.NetworkCredential]::new('', $secret).Password) -RunLevel Limited | Out-Null
    $registered = $true
    Start-ScheduledTask -TaskName $taskName
    $started = [DateTime]::UtcNow
    $deadline = $started.AddMinutes(25)
    $nextProgress = $started.AddSeconds(15)
    $resultPath = Join-Path $root 'result'
    while (-not (Test-Path $resultPath)) {
        if ([DateTime]::UtcNow -gt $nextProgress) {
            $task = Get-ScheduledTask -TaskName $taskName
            $info = Get-ScheduledTaskInfo -TaskName $taskName
            Write-Output "Native acceptance task: $($task.State); Task Scheduler result: $($info.LastTaskResult)"
            $log = Join-Path $root 'acceptance.log'
            if (Test-Path $log) { Get-Content $log -Tail 5 -ErrorAction SilentlyContinue }
            if ($task.State -ne 'Running' -and -not (Test-Path $resultPath)) {
                throw "Native acceptance host did not remain running; Task Scheduler result: $($info.LastTaskResult)"
            }
            $nextProgress = [DateTime]::UtcNow.AddSeconds(30)
        }
        if ([DateTime]::UtcNow -gt $deadline) {
            $info = Get-ScheduledTaskInfo -TaskName $taskName
            throw "Native acceptance timed out; Task Scheduler result: $($info.LastTaskResult)"
        }
        Start-Sleep -Milliseconds 500
    }
    $reported = [IO.File]::ReadAllText($resultPath)
    if ($reported -notmatch '^-?\d+$') { throw 'Native acceptance did not publish an exit code.' }
    $result = [int]$reported
} finally {
    $log = Join-Path $root 'acceptance.log'
    if (Test-Path $log) { Write-Output ([IO.File]::ReadAllText($log)) }
    if ($registered) {
        Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    }
    if ($created) {
        # Reap only processes belonging to the account created by this run.
        foreach ($process in Get-CimInstance Win32_Process) {
            $owner = Invoke-CimMethod -InputObject $process -MethodName GetOwnerSid -ErrorAction SilentlyContinue
            if ($owner.Sid -eq $created.SID.Value) {
                Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-LocalUser -SID $created.SID -ErrorAction SilentlyContinue
        Get-CimInstance Win32_UserProfile -Filter "SID='$($created.SID.Value)'" | Remove-CimInstance -ErrorAction SilentlyContinue
    }
    if (Test-Path $root) { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}
exit $result
