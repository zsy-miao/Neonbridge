#requires -Version 5.1
<#
.SYNOPSIS
Lance les tests GPU d'AnyXboxX sur un adaptateur et écrit un rapport Markdown par carte (issue #70).

.DESCRIPTION
L'adaptateur est celui que choisit anyxboxx_gpu_run.exe : par défaut le premier adaptateur de DXGI,
-Adapter <partie du nom> pour une autre carte, -Backend warp pour le rendu logiciel de Windows.
Le rapport (par défaut %LOCALAPPDATA%\AnyXboxX\banc-gpu\rapport-<carte>.md) se colle dans le fil des rapports GPU.
Il faut le runtime C++ compilé (runtime/build), dxc du SDK Windows et l'environnement Python de tests (.venv).
Un nom d'adaptateur qui ne correspond à aucune carte, ou dxc absent : arrêt sans rapport (code 1).

.EXAMPLE
.\tools\Run-GpuCheck.ps1

.EXAMPLE
.\tools\Run-GpuCheck.ps1 -Adapter Radeon

.EXAMPLE
.\tools\Run-GpuCheck.ps1 -Backend warp

.NOTES
Adapté de tools/gpu-check/Run-GpuCheck.ps1 de Xbox2Windows (https://github.com/R0Y00/Xbox2Windows, commit bcdce96),
licence MIT, Copyright (c) 2026 Xbox2Windows contributors ; texte complet dans third_party/xbox2windows/LICENSE.
#>
[CmdletBinding()]
param(
    [ValidateSet('hardware', 'warp')]
    [string]$Backend = 'hardware',
    [string]$Adapter,
    [string]$ReportPath,
    [string]$Python
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$gpuTests = 'tests/test_rdna2_hlsl.py', 'tests/test_rdna2_graphics.py', 'tests/test_rdna2_image.py'
$thread = 'https://github.com/LooperSalty/AnyXboxX/issues/112'

# Lance un programme sans fenêtre, ANYXBOXX_GPU fixé (ou retiré), sorties lues en UTF-8.
function Invoke-Tool {
    param([string]$FilePath, [string[]]$Arguments, [string]$Gpu, [int]$Timeout)
    $process = [System.Diagnostics.Process]::new()
    $info = $process.StartInfo
    $info.FileName = $FilePath
    $info.Arguments = ($Arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $info.WorkingDirectory = $root
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = [Text.Encoding]::UTF8
    $info.StandardErrorEncoding = [Text.Encoding]::UTF8
    if ($Gpu) { $info.EnvironmentVariables['ANYXBOXX_GPU'] = $Gpu } else { $info.EnvironmentVariables.Remove('ANYXBOXX_GPU') }
    try {
        [void]$process.Start()
        $out = $process.StandardOutput.ReadToEndAsync()
        $err = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit($Timeout)
        if ($timedOut) { $process.Kill() }
        $process.WaitForExit()
        $errors = $err.Result
        if ($timedOut) { $errors += "`ndélai dépassé ($($Timeout / 1000) s)" }
        [pscustomobject]@{ Code = $process.ExitCode; Out = $out.Result; Err = $errors }
    } finally {
        $process.Dispose()
    }
}

function Get-Lines([string]$Text) { @($Text -split "`r?`n" | Where-Object { $_ }) }

$runtime = if ($env:ANYXBOXX_RUNTIME) { $env:ANYXBOXX_RUNTIME } else { Join-Path $root 'runtime\build\bin' }
$runner = Join-Path $runtime 'anyxboxx_gpu_run.exe'
if (-not (Test-Path -LiteralPath $runner -PathType Leaf)) {
    Write-Output 'Runtime C++ absent : cmake -S runtime -B runtime/build puis cmake --build runtime/build --config Release'
    exit 1
}
if (-not $Python) {
    $venv = Join-Path $root '.venv\Scripts\python.exe'
    $Python = if (Test-Path -LiteralPath $venv) { $venv } else { 'python' }
}
$gpu = if ($Backend -eq 'warp') { 'warp' } elseif ($Adapter) { $Adapter } else { '' }

$description = Invoke-Tool -FilePath $runner -Arguments '--adaptateur' -Gpu $gpu -Timeout 60000
if ($description.Code -ne 0) {
    Get-Lines ($description.Err + "`n" + $description.Out) | Write-Output
    exit 1
}
$fields = @{}
foreach ($line in Get-Lines $description.Out) {
    if ($line -match '^adapter=(.+) vendor=([0-9a-f]+) device=([0-9a-f]+)$') {
        $fields.adapter = $Matches[1]; $fields.vendor = $Matches[2]; $fields.device = $Matches[3]
    } elseif ($line -match '^(\w+)=(.*)$') {
        $fields[$Matches[1]] = $Matches[2]
    }
}

# Même recherche que emu/rdna2_gpu.find_dxc : sans dxc, les tests GPU seraient seulement ignorés.
$kits = if (${env:ProgramFiles(x86)}) { ${env:ProgramFiles(x86)} } else { 'C:\Program Files (x86)' }
$dxc = if ($env:ANYXBOXX_DXC) { $env:ANYXBOXX_DXC } else {
    Get-ChildItem -Path (Join-Path $kits 'Windows Kits\10\bin\10.*\x64\dxc.exe') -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $dxc -or -not (Test-Path -LiteralPath $dxc -PathType Leaf)) {
    Write-Output 'dxc absent : installe le SDK Windows 10.0.26100 (dxc.exe) ou définis ANYXBOXX_DXC'
    exit 1
}

$junit = Join-Path ([IO.Path]::GetTempPath()) ('anyxboxx-banc-gpu-{0}.xml' -f [guid]::NewGuid())
$arguments = @('-m', 'pytest') + $gpuTests + @('-q', '-p', 'no:cacheprovider', "--junitxml=$junit")
try {
    $run = Invoke-Tool -FilePath $Python -Arguments $arguments -Gpu $gpu -Timeout 600000
} catch {
    $run = [pscustomobject]@{ Code = -1; Out = ''; Err = $_.Exception.Message }
}
$cases = @()
if (Test-Path -LiteralPath $junit) {
    [xml]$xml = [IO.File]::ReadAllText($junit)
    Remove-Item -LiteralPath $junit
    $cases = @(foreach ($case in $xml.SelectNodes('//testcase')) {
        $problem = $case.SelectSingleNode('failure|error')
        $skipped = $case.SelectSingleNode('skipped')
        $node = if ($problem) { $problem } else { $skipped }
        [pscustomobject]@{
            Name = '{0}::{1}' -f ($case.GetAttribute('classname') -split '\.')[-1], $case.GetAttribute('name')
            Status = $(if ($problem) { 'FAIL' } elseif ($skipped) { 'SKIP' } else { 'PASS' })
            Details = $(if ($node) { ($node.GetAttribute('message') + "`n" + $node.InnerText).Trim() } else { '' })
        }
    })
}
if ($cases.Count -eq 0) {
    Write-Output ("Les tests n'ont pas pu tourner (code {0}) :" -f $run.Code)
    Get-Lines ($run.Out + "`n" + $run.Err) | Select-Object -Last 20 | Write-Output
    exit 1
}

if (-not $ReportPath) {
    $name = if ($fields.adapter) { $fields.adapter } else { $Backend }
    $slug = ($name.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-')
    $base = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { $HOME }
    $ReportPath = Join-Path $base "AnyXboxX\banc-gpu\rapport-$slug.md"
}
$ReportPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($ReportPath)
$windows = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$commit = try { (Invoke-Tool -FilePath 'git' -Arguments 'rev-parse', 'HEAD' -Timeout 30000).Out.Trim() } catch { '' }

$report = [System.Collections.Generic.List[string]]::new()
$report.Add("## Rapport GPU d'AnyXboxX")
$report.Add('')
$report.Add("Commit : $(if ($commit) { $commit } else { 'inconnu' })")
$report.Add("Windows : $($windows.CurrentBuildNumber).$($windows.UBR)")
$report.Add("Backend : $($fields.backend)")
if ($Backend -eq 'hardware' -and $Adapter) { $report.Add("Filtre d'adaptateur : $Adapter") }
$report.Add('')
$report.Add('### Adaptateur')
$report.Add('')
$report.Add("- Adaptateur : $($fields.adapter) (vendor=$($fields.vendor) device=$($fields.device))")
$report.Add("- Pilote : $($fields.driver)")
$report.Add("- Largeur de wave native : $($fields.native_lane_range)")
$report.Add("- Ray tracing (DXR) : $($fields.raytracing_tier)")
$report.Add("- Double précision dans les shaders : $($fields.double_precision)")
$report.Add('')
$report.Add('| Test | Résultat |')
$report.Add('|---|---|')
foreach ($case in $cases) { $report.Add("| $($case.Name) | $($case.Status) |") }
foreach ($case in $cases | Where-Object Status -ne 'PASS') {
    $report.Add('')
    $report.Add("### $($case.Name) - $($case.Status)")
    $report.Add('')
    $report.Add('```text')
    $report.Add($case.Details)
    $report.Add('```')
}
$report.Add('')
$report.Add('### Envoyer ce rapport')
$report.Add('')
$report.Add("Colle tout ce fichier en commentaire de $thread.")

[void](New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ReportPath))
[IO.File]::WriteAllLines($ReportPath, $report, [Text.UTF8Encoding]::new($false))
$report | Write-Output
Write-Output "Rapport enregistré : $ReportPath"
$failed = @($cases | Where-Object Status -eq 'FAIL').Count -gt 0
exit $(if ($failed) { 1 } else { 0 })
