# Mediciones de rendimiento de YoutubeInRustWeb (ver src/bench.rs).
#   powershell -Command "& .\scripts\bench.ps1 [-Exe target\release\YoutubeInRustWeb.exe] [-Runs 5] [-Size 1920x1040] [-Scenarios start,scroll,play]"
#   (con -File, "-Scenarios a,b" llega como un solo texto)
# Usa una carpeta de datos aparte (no toca la configuracion real) y copia ahi yt-dlp.
param(
    [string]$Exe = "target\release\YoutubeInRustWeb.exe",
    [int]$Runs = 5,
    [string[]]$Scenarios = @("start", "scroll", "play"),
    [string]$Size = ""
)
$ErrorActionPreference = "Stop"
$exePath = (Resolve-Path $Exe).Path
$work = Join-Path $env:TEMP "yir-bench"
$data = Join-Path $work "data"
New-Item -ItemType Directory -Force (Join-Path $data "bin") | Out-Null
$realBin = Join-Path $env:APPDATA "YoutubeInRustWeb\bin"
if ((Test-Path $realBin) -and -not (Test-Path (Join-Path $data "bin\yt-dlp"))) {
    Copy-Item -Recurse -Force "$realBin\*" (Join-Path $data "bin")
}

function UnixUs { [long](([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()) * 1000) }
function Pct($xs, $p) {
    if (-not $xs -or $xs.Count -eq 0) { return $null }
    $s = @($xs | Sort-Object)
    $s[[math]::Min($s.Count - 1, [math]::Floor($s.Count * $p))]
}
function Ms($us) { if ($null -eq $us) { "-" } else { "{0:N1}" -f ($us / 1000.0) } }
function Mb($b) { if ($null -eq $b) { "-" } else { "{0:N1}" -f ($b / 1MB) } }

function Run-Once($scenario, $index) {
    $file = Join-Path $work "$scenario-$([guid]::NewGuid().ToString('N')).txt"
    $env:YIR_DATA_DIR = $data
    $env:YIR_BENCH_FILE = $file
    $env:YIR_BENCH = $scenario
    $env:YIR_BENCH_INDEX = "$index"
    if ($Size) { $env:YIR_BENCH_SIZE = $Size } else { Remove-Item Env:YIR_BENCH_SIZE -ErrorAction SilentlyContinue }
    # Sin sesion guardada ni cola anterior: cada corrida parte igual.
    Remove-Item (Join-Path $data "session.json") -ErrorAction SilentlyContinue
    $t0 = UnixUs
    $p = Start-Process -FilePath $exePath -PassThru
    $mem = New-Object System.Collections.ArrayList
    while (-not $p.HasExited) {
        try {
            $p.Refresh()
            [void]$mem.Add([pscustomobject]@{ t = (UnixUs); ws = $p.WorkingSet64; priv = $p.PrivateMemorySize64 })
        } catch {}
        Start-Sleep -Milliseconds 200
        if (((UnixUs) - $t0) -gt 120000000) { Stop-Process $p -Force; break }
    }
    Remove-Item Env:YIR_BENCH, Env:YIR_BENCH_FILE, Env:YIR_DATA_DIR, Env:YIR_BENCH_INDEX -ErrorAction SilentlyContinue

    $frames = @(); $ev = @{}; $evAll = @()
    foreach ($line in Get-Content $file) {
        $a = $line.Split(" ")
        if ($a[0] -eq "f") { $frames += [pscustomobject]@{ t = [long]$a[1]; us = [long]$a[2]; w = [int]$a[3]; h = [int]$a[4] } }
        elseif ($a[0] -eq "e") {
            $o = [pscustomobject]@{ t = [long]$a[1]; name = $a[2]; v = [long]$a[3] }
            $evAll += $o
            if (-not $ev.ContainsKey($o.name)) { $ev[$o.name] = $o }
        }
    }
    function FramesIn($a, $b) { @($frames | Where-Object { $_.t -ge $a -and $_.t -le $b }) }
    function MemIn($a, $b) { @($mem | Where-Object { $_.t -ge $a -and $_.t -le $b }) }
    $r = [ordered]@{ scenario = $scenario }
    if ($frames.Count) {
        $r.first_frame_ms = ($frames[0].t - $t0) / 1000
        $r.size = "$($frames[-1].w)x$($frames[-1].h)"
    }
    if ($ev.home_rows) { $r.home_rows = $ev.home_rows.v; $r.home_rows_ms = ($ev.home_rows.t - $t0) / 1000 }
    if ($ev.home_covers_missing) { $r.home_covers_ms = ($ev.home_covers_missing.t - $t0) / 1000; $r.covers_missing = $ev.home_covers_missing.v }
    $dec = @($evAll | Where-Object { $_.name -eq "cover_decode_us" } | ForEach-Object { $_.v })
    if ($dec.Count) { $r.cover_decode_p50_ms = (Pct $dec 0.5) / 1000; $r.covers_decoded = $dec.Count }
    $rows = @($evAll | Where-Object { $_.name -eq "row_sets_1s" } | ForEach-Object { $_.v })
    if ($rows.Count) { $r.row_sets_max_1s = ($rows | Measure-Object -Maximum).Maximum }
    $inv = @($evAll | Where-Object { $_.name -eq "invokes_1s" } | ForEach-Object { $_.v })
    if ($inv.Count) { $r.invokes_max_1s = ($inv | Measure-Object -Maximum).Maximum }

    $win = $null
    if ($ev.idle_begin -and $ev.idle_end) { $win = @($ev.idle_begin.t, $ev.idle_end.t) }
    if ($ev.scroll_begin -and $ev.scroll_end) { $win = @($ev.scroll_begin.t, $ev.scroll_end.t) }
    if ($ev.playing -and $ev.play_end) { $win = @($ev.playing.t, $ev.play_end.t) }
    if ($ev.click_queue -and $ev.page_changed) {
        $after = @($frames | Where-Object { $_.t -ge $ev.page_changed.t })
        if ($after.Count) { $r.click_to_frame_ms = ($after[0].t - $ev.click_queue.t) / 1000 }
    }
    if ($ev.play_cmd -and $ev.playing) { $r.play_to_sound_ms = ($ev.playing.t - $ev.play_cmd.t) / 1000 }
    # Clic a sonido por tramos (ms desde el clic; ver src/bench.rs y src/stream.rs).
    if ($ev.play_cmd) {
        $r.song_index = $ev.play_cmd.v
        foreach ($k in "lat_load_start", "lat_t1_resolve_start", "lat_t2_resolved", "lat_t3_http_headers", "lat_t4_first_chunk", "lat_engine_loaded", "lat_first_sample", "lat_first_loud_sample") {
            if ($ev[$k]) { $r[$k] = [math]::Round(($ev[$k].t - $ev.play_cmd.t) / 1000, 1) }
        }
        if ($ev.lat_t2_resolved) { $r.resolve_cached = $ev.lat_t2_resolved.v }
        if ($ev.lat_t4_first_chunk) { $r.first_chunk_bytes = $ev.lat_t4_first_chunk.v }
    }
    if ($win) {
        $secs = ($win[1] - $win[0]) / 1e6
        $f = FramesIn $win[0] $win[1]
        $us = @($f | ForEach-Object { $_.us })
        $r.window_s = [math]::Round($secs, 1)
        $r.fps = [math]::Round($f.Count / $secs, 1)
        $r.frame_p50_ms = (Pct $us 0.5) / 1000
        $r.frame_p95_ms = (Pct $us 0.95) / 1000
        $r.frame_max_ms = (($us | Measure-Object -Maximum).Maximum) / 1000
        $m = MemIn $win[0] $win[1]
        if ($m.Count) {
            $r.ws_mb = [math]::Round((Pct @($m | ForEach-Object { $_.ws }) 0.5) / 1MB, 1)
            $r.priv_mb = [math]::Round((Pct @($m | ForEach-Object { $_.priv }) 0.5) / 1MB, 1)
        }
    }
    if ($mem.Count) {
        $r.ws_peak_mb = [math]::Round((($mem | Measure-Object ws -Maximum).Maximum) / 1MB, 1)
        $r.priv_peak_mb = [math]::Round((($mem | Measure-Object priv -Maximum).Maximum) / 1MB, 1)
    }
    if ($ev.timeout_home) { $r.error = "timeout_home" }
    if ($ev.timeout_play) { $r.error = "timeout_play" }
    [pscustomobject]$r
}

$all = @()
foreach ($s in $Scenarios) {
    for ($i = 1; $i -le $Runs; $i++) {
        Write-Host "[$s $i/$Runs]"
        $all += Run-Once $s ($i - 1)
    }
}
$all | Format-List
$all | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $work "results.json")
Write-Host "Resultados: $(Join-Path $work 'results.json')"
