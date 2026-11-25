# Samples memory every 2 s while victory.exe runs.
param([string]$Out = 'D:\Neptune-VE\memwatch.csv')
$csv = $Out
'time,private_gb,working_gb,ram_free_gb,commit_free_gb,pagefile_gb,hard_faults_per_s,vram_used_gb' | Set-Content $csv
while (-not (Get-Process victory -ErrorAction SilentlyContinue)) { Start-Sleep 1 }
while ($p = Get-Process victory -ErrorAction SilentlyContinue | Select-Object -First 1) {
  $os = Get-CimInstance Win32_OperatingSystem
  $pf = (Get-CimInstance Win32_PageFileUsage | Measure-Object CurrentUsage -Sum).Sum / 1024
  $faults = (Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory).PagesInputPersec
  $vram = (& nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits) / 1024
  [string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0:HH:mm:ss},{1:F2},{2:F2},{3:F2},{4:F2},{5:F2},{6:F0},{7:F2}', (Get-Date), ($p.PrivateMemorySize64/1GB), ($p.WorkingSet64/1GB), ($os.FreePhysicalMemory/1MB), ($os.FreeVirtualMemory/1MB), $pf, $faults, $vram) | Add-Content $csv
  Start-Sleep 2
}
