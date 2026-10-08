param([Parameter(Mandatory)][string]$Companion, [switch]$PreflightOnly)

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
    $arguments = @((Join-Path $PSScriptRoot 'test-tui-instances.py'), '--companion', $Companion)
    if ($PreflightOnly) { $arguments += '--preflight-only' }
    & python3 @arguments
    exit $LASTEXITCODE
}

$suffix = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$name = 'tui-' + $suffix
$root = Join-Path $env:SystemDrive ('t' + $suffix)
$brokerPid = $null
$serviceName = $null
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
    $directories = @($PSScriptRoot)
    if (-not $PreflightOnly) { $directories += (Split-Path $companionPath) }
    foreach ($directory in $directories) {
        & icacls.exe $directory /grant "${sid}:(OI)(CI)RX" /T /Q | Out-Null
        if ($LASTEXITCODE) { throw 'Cannot grant fixture access to test binaries.' }
    }
    $launch = @{
        python = (Get-Command python3).Source
        script = Join-Path $PSScriptRoot 'test-tui-instances.py'
        companion = $companionPath
        fixture = Join-Path $root 'f'
        github_actions = $env:GITHUB_ACTIONS
        preflight_only = [bool]$PreflightOnly
        path = $env:Path
    }
    [IO.File]::WriteAllText((Join-Path $root 'launch.json'), ($launch | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    # An owned temporary service supplies a host outside GitHub's and
    # Secondary Logon's jobs, with the OS right to use the standard token.
    $credentialPath = Join-Path $root 'credential.json'
    [IO.File]::WriteAllText($credentialPath, (@{user = "$env:COMPUTERNAME\$name"; password = ([Net.NetworkCredential]::new('', $secret).Password)} | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    $broker = Join-Path $PSScriptRoot 'test-tui-windows-host.py'
    $serviceName = 'ac-tui-' + $suffix
    $binary = "`"$($launch.python)`" `"$broker`" `"$root`" --service $serviceName"
    New-Service -Name $serviceName -BinaryPathName $binary -StartupType Manual -Description 'Disposable native TUI acceptance host' | Out-Null
    Start-Service -Name $serviceName
    $brokerPid = (Get-CimInstance Win32_Service -Filter "Name='$serviceName'").ProcessId
    $started = [DateTime]::UtcNow
    $deadline = if ($PreflightOnly) { $started.AddMinutes(1) } else { $started.AddMinutes(25) }
    $nextProgress = $started.AddSeconds(15)
    $resultPath = Join-Path $root 'result'
    while (-not (Test-Path $resultPath)) {
        if ([DateTime]::UtcNow -gt $nextProgress) {
            Write-Output 'Native acceptance host is running under the disposable standard account.'
            $log = Join-Path $root 'stdout.log'
            if (Test-Path $log) { Get-Content $log -Tail 5 -ErrorAction SilentlyContinue }
            $log = Join-Path $root 'acceptance.log'
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
        if (Test-Path $log) {
            # A failed/timed-out fixture can still have its log open. Reading
            # must permit its writer, and must never prevent owned cleanup.
            try {
                $stream = [IO.File]::Open($log, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
                $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8)
                try { Write-Output ($reader.ReadToEnd()) } finally { $reader.Dispose() }
            } catch { Write-Warning "Cannot read native fixture diagnostic $name" }
        }
    }
    if ($brokerPid) {
        $process = Get-CimInstance Win32_Process -Filter "ProcessId=$brokerPid"
        if ($process.Name -like 'python*.exe' -and $process.CommandLine.Contains($broker) -and $process.CommandLine.Contains($root)) {
            Stop-Process -Id $brokerPid -Force -ErrorAction SilentlyContinue
        }
    }
    if ($serviceName) {
        Stop-Service -Name $serviceName -ErrorAction SilentlyContinue
        & sc.exe delete $serviceName | Out-Null
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
