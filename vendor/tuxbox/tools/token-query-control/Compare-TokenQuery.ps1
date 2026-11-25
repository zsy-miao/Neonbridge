# Run from an unpackaged x64 Windows PowerShell session.
[CmdletBinding()]
param(
    [string]$OutputPath = 'token-query-windows.json',
    [ValidateSet('unspecified', 'physical', 'virtual-machine')]
    [string]$HostKind = 'unspecified'
)
$ErrorActionPreference = 'Stop'

function Read-Probe([string]$Executable) {
    $receipt = Join-Path $PSScriptRoot ("probe-{0}.json" -f [guid]::NewGuid())
    try {
        & $Executable $receipt
        if ($LASTEXITCODE -ne 0) { throw "Probe exited with code $LASTEXITCODE" }
        return (Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json)
    } finally {
        if (Test-Path -LiteralPath $receipt) { Remove-Item -LiteralPath $receipt }
    }
}

$name = 'ProjectTuxBox.TokenQuery'

# Hold a per-user lock across the initial check, registration and cleanup.
$lockPath = Join-Path $env:LOCALAPPDATA "$name.control.lock"
$lock = [System.IO.FileStream]::new(
    $lockPath, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite,
    [System.IO.FileShare]::None, 1, [System.IO.FileOptions]::DeleteOnClose
)
try {
    $layout = Join-Path $PSScriptRoot 'package'
    $exe = Join-Path $layout 'token-query.exe'
    if (Get-AppxPackage -Name $name) { throw "Package $name is already registered for this user." }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash.ToLowerInvariant()
    $plain = Read-Probe $exe
    if ($plain.package_status -ne 15700) { throw 'Expected APPMODEL_ERROR_NO_PACKAGE from the ordinary process.' }

    try {
        Add-AppxPackage -Register (Join-Path $layout 'AppxManifest.xml')
        $pkg = Get-AppxPackage -Name $name
        if (!$pkg -or $pkg.InstallLocation -ne $layout) { throw "Unexpected registration location for $name." }
        $alias = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\tuxbox-token-query.exe'
        $packaged = Read-Probe $alias
        if ($packaged.package_status -ne 0 -or $packaged.package_full_name -cne $pkg.PackageFullName) {
            throw 'Activated probe did not report the registered package identity.'
        }
        $provider = "Windows x64 $((Get-CimInstance Win32_OperatingSystem).Version)"
        $plain | Add-Member -NotePropertyName activation -NotePropertyValue direct
        $packaged | Add-Member -NotePropertyName activation -NotePropertyValue package_alias
        $result = [ordered]@{
            type = 'host-capability'
            host_kind = $HostKind
            probe = 'NtQueryInformationToken'
            target = 'current process token'
            information_class = 39
            buffer_bytes = 0
            exe_sha256 = $hash
            observations = @(foreach ($run in @($plain, $packaged)) {
                $run | Add-Member -NotePropertyMembers @{
                    backend = 'native'
                    provider = $provider
                } -PassThru
            })
        }
        $result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding Ascii
        Get-Item -LiteralPath $OutputPath
    } finally {
        $registered = Get-AppxPackage -Name $name
        if ($registered -and $registered.InstallLocation -eq $layout) {
            Remove-AppxPackage -Package $registered.PackageFullName
        }
    }
} finally {
    $lock.Dispose()
}
