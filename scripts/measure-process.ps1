param(
    [Parameter(Mandatory=$true)][int]$ProcessId,
    [ValidateRange(2,86400)][int]$Seconds = 120,
    [string]$Output = "process-measurement.csv"
)
$ErrorActionPreference = "Stop"
$rows = [System.Collections.Generic.List[object]]::new()
$watch = [System.Diagnostics.Stopwatch]::StartNew()
$process = Get-Process -Id $ProcessId
$initialCpu = $process.TotalProcessorTime.TotalSeconds
$cores = [Environment]::ProcessorCount
for ($i = 0; $i -le $Seconds; $i++) {
    $process.Refresh()
    if ($process.HasExited) { throw "Process exited during measurement" }
    $rows.Add([pscustomobject]@{
        ElapsedSeconds = $watch.Elapsed.TotalSeconds
        WorkingSetMiB = $process.WorkingSet64 / 1MB
        PrivateMiB = $process.PrivateMemorySize64 / 1MB
        CpuSeconds = $process.TotalProcessorTime.TotalSeconds - $initialCpu
        Threads = $process.Threads.Count
        Handles = $process.HandleCount
    })
    if ($i -lt $Seconds) { Start-Sleep -Seconds 1 }
}
$rows | Export-Csv -NoTypeInformation -Encoding utf8 -Path $Output
$last = $rows[$rows.Count - 1]
[pscustomobject]@{
    Samples = $rows.Count
    DurationSeconds = [math]::Round($last.ElapsedSeconds, 2)
    MeanWorkingSetMiB = [math]::Round(($rows.WorkingSetMiB | Measure-Object -Average).Average, 3)
    MeanPrivateMiB = [math]::Round(($rows.PrivateMiB | Measure-Object -Average).Average, 3)
    CpuSeconds = [math]::Round($last.CpuSeconds, 4)
    CpuPercent = [math]::Round(100 * $last.CpuSeconds / $last.ElapsedSeconds / $cores, 4)
    LogicalProcessors = $cores
    Output = $Output
}
