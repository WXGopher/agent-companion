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
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if ([Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Native daemon acceptance must use a standard user.'
}
$launch = Get-Content (Join-Path $PSScriptRoot 'launch.json') -Raw | ConvertFrom-Json
$env:Path = $launch.path
$env:TEMP = Join-Path $PSScriptRoot 'tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Path $env:TEMP | Out-Null
& $launch.python $launch.script --companion $launch.companion --work-dir $launch.fixture
exit $LASTEXITCODE
'@, [Text.UTF8Encoding]::new($false))
    $credential = [PSCredential]::new("$env:COMPUTERNAME\$name", $secret)
    $shell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $child = Start-Process -FilePath $shell -Credential $credential -LoadUserProfile -WorkingDirectory $root -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', "`"$bootstrap`"") -RedirectStandardOutput (Join-Path $root 'stdout.log') -RedirectStandardError (Join-Path $root 'stderr.log') -Wait -PassThru
    $result = $child.ExitCode
    foreach ($log in @('stdout.log', 'stderr.log')) {
        Write-Output ([IO.File]::ReadAllText((Join-Path $root $log)))
    }
} finally {
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
