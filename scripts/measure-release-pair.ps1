param(
    [Parameter(Mandatory=$true)][string]$BaselineExe,
    [Parameter(Mandatory=$true)][string]$ConnectExe,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [ValidateRange(2,3600)][int]$Seconds = 120
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$oldAppData = $env:APPDATA
$summaries = @()
try {
    foreach ($variant in @(
        @{ Name = "baseline-logged-out-tray"; Exe = $BaselineExe; Enabled = $false },
        @{ Name = "connect-disabled-logged-out-tray"; Exe = $ConnectExe; Enabled = $false },
        @{ Name = "connect-enabled-no-account-tray"; Exe = $ConnectExe; Enabled = $true }
    )) {
        $profile = Join-Path $OutputDirectory $variant.Name
        $settings = Join-Path $profile "yamusic-rs"
        New-Item -ItemType Directory -Force -Path $settings | Out-Null
        @{
            discord_enabled = $false
            check_updates = $false
            wave_motion = $false
            mode = "lite"
            ynison_enabled = $variant.Enabled
            ynison_name = "Measurement"
        } | ConvertTo-Json | Set-Content -Encoding utf8NoBOM -Path (Join-Path $settings "config.json")
        $env:APPDATA = $profile
        $process = Start-Process -FilePath $variant.Exe -ArgumentList "--tray" -PassThru
        try {
            Start-Sleep -Seconds 30
            $process.Refresh()
            if ($process.HasExited) { throw "$($variant.Name) exited before measurement" }
            $result = & "$PSScriptRoot/measure-process.ps1" -ProcessId $process.Id -Seconds $Seconds -Output (Join-Path $OutputDirectory "$($variant.Name).csv")
            $summaries += $result | Select-Object @{Name="Variant";Expression={$variant.Name}}, *
        } finally {
            # Only stop the exact disposable process created by this script.
            $process.Refresh()
            if (-not $process.HasExited) {
                if (-not $process.CloseMainWindow()) { $process.Kill() }
                if (-not $process.WaitForExit(5000)) { $process.Kill() }
            }
        }
    }
} finally {
    $env:APPDATA = $oldAppData
}
$summaries | Export-Csv -Encoding utf8 -NoTypeInformation -Path (Join-Path $OutputDirectory "summary.csv")
$summaries | Format-Table -AutoSize
