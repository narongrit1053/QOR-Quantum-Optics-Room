param([int]$ProcessId, [int]$Seconds = 10)
# Measures CPU and GPU use of an idle process. Touches nothing.
$p = Get-Process -Id $ProcessId
$cpu0 = $p.TotalProcessorTime
$t0 = Get-Date
$samples = Get-Counter -Counter "\GPU Engine(pid_$($ProcessId)_*)\Utilization Percentage" -SampleInterval 1 -MaxSamples $Seconds -ErrorAction SilentlyContinue
$p.Refresh()
$elapsed = ((Get-Date) - $t0).TotalSeconds
$cpuPct = ($p.TotalProcessorTime - $cpu0).TotalSeconds / $elapsed / [Environment]::ProcessorCount * 100
$gpu = @()
foreach ($s in $samples) { $gpu += ($s.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum }
$gpuAvg = if ($gpu.Count) { ($gpu | Measure-Object -Average).Average } else { 0 }
$gpuMax = if ($gpu.Count) { ($gpu | Measure-Object -Maximum).Maximum } else { 0 }
"Idle over {0:N1} s: CPU {1:N2}% of machine, GPU avg {2:N2}% / max {3:N2}% (all engines), working set {4:N0} MB" -f `
  $elapsed, $cpuPct, $gpuAvg, $gpuMax, ($p.WorkingSet64 / 1MB)
