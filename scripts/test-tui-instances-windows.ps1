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
$brokerPid = $null
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
    $env:TEMP = Join-Path $PSScriptRoot 'tmp'
    $env:TMP = $env:TEMP
    New-Item -ItemType Directory -Path $env:TEMP | Out-Null
    # Avoid PS5 turning native stderr into a terminating NativeCommandError.
    # Capture raw UTF-8 streams and wait on this process, without -Wait's job.
    $child = Start-Process -FilePath $launch.python -WorkingDirectory $PSScriptRoot -ArgumentList @("`"$($launch.script)`"", '--companion', "`"$($launch.companion)`"", '--work-dir', "`"$($launch.fixture)`"") -RedirectStandardOutput (Join-Path $PSScriptRoot 'stdout.log') -RedirectStandardError (Join-Path $PSScriptRoot 'stderr.log') -PassThru
    $null = $child.Handle
    $child.WaitForExit()
    $result = $child.ExitCode
    if ($null -eq $result) { throw 'Native Python acceptance did not report an exit code.' }
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
    # Win32_Process.Create supplies a host outside GitHub's restrictive job.
    # The broker immediately switches to the disposable standard account.
    # Production launch flags and the runner's job are never changed.
    # WMI applies its own quota job unless the startup requests breakaway.
    # https://learn.microsoft.com/windows/win32/cimwin32prov/create-method-in-class-win32-process
    $credentialPath = Join-Path $root 'credential.json'
    [IO.File]::WriteAllText($credentialPath, (@{user = "$env:COMPUTERNAME\$name"; password = ([Net.NetworkCredential]::new('', $secret).Password)} | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    $broker = Join-Path $root 'broker.ps1'
    [IO.File]::WriteAllText($broker, @'
$ErrorActionPreference = 'Stop'
try {
    $credentialPath = Join-Path $PSScriptRoot 'credential.json'
    $login = Get-Content $credentialPath -Raw | ConvertFrom-Json
    Remove-Item $credentialPath
    $credential = [PSCredential]::new($login.user, (ConvertTo-SecureString $login.password -AsPlainText -Force))
    $login = $null
    $shell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $bootstrap = Join-Path $PSScriptRoot 'run.ps1'
    # -Wait would add another restrictive process-tree job. Wait on this
    # single handle; the fixture owns native daemon cleanup.
    $child = Start-Process -FilePath $shell -Credential $credential -LoadUserProfile -WorkingDirectory $PSScriptRoot -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', "`"$bootstrap`"") -PassThru
    $null = $child.Handle
    $child.WaitForExit()
} catch {
    $_ | Out-File -FilePath (Join-Path $PSScriptRoot 'acceptance.log') -Encoding utf8 -Append
    $pending = Join-Path $PSScriptRoot 'result.pending'
    [IO.File]::WriteAllText($pending, '1')
    [IO.File]::Move($pending, (Join-Path $PSScriptRoot 'result'))
    exit 1
}
'@, [Text.UTF8Encoding]::new($false))
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{CreateFlags = [uint32]0x01000000}
    $spawn = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine = "`"$shell`" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$broker`""; CurrentDirectory = $root; ProcessStartupInformation = $startup}
    if ($spawn.ReturnValue -ne 0) { throw "Cannot start native acceptance host: $($spawn.ReturnValue)" }
    $brokerPid = $spawn.ProcessId
    $started = [DateTime]::UtcNow
    $deadline = $started.AddMinutes(25)
    $nextProgress = $started.AddSeconds(15)
    $resultPath = Join-Path $root 'result'
    while (-not (Test-Path $resultPath)) {
        if ([DateTime]::UtcNow -gt $nextProgress) {
            Write-Output 'Native acceptance host is running under the disposable standard account.'
            $log = Join-Path $root 'stdout.log'
            if (Test-Path $log) { Get-Content $log -Tail 5 -ErrorAction SilentlyContinue }
            if (-not (Get-Process -Id $brokerPid -ErrorAction SilentlyContinue) -and -not (Test-Path $resultPath)) {
                throw 'Native acceptance host exited without reporting a result.'
            }
            $nextProgress = [DateTime]::UtcNow.AddSeconds(30)
        }
        if ([DateTime]::UtcNow -gt $deadline) {
            throw 'Native acceptance timed out.'
        }
        Start-Sleep -Milliseconds 500
    }
    $reported = [IO.File]::ReadAllText($resultPath)
    if ($reported -notmatch '^-?\d+$') { throw 'Native acceptance did not publish an exit code.' }
    $result = [int]$reported
} finally {
    foreach ($name in @('stdout.log', 'stderr.log', 'acceptance.log')) {
        $log = Join-Path $root $name
        if (Test-Path $log) { Write-Output ([IO.File]::ReadAllText($log)) }
    }
    if ($brokerPid) {
        $process = Get-CimInstance Win32_Process -Filter "ProcessId=$brokerPid"
        if ($process.Name -eq 'powershell.exe' -and $process.CommandLine.Contains($broker)) {
            Stop-Process -Id $brokerPid -Force -ErrorAction SilentlyContinue
        }
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
