# Copyright 2026 SEIZIA (Jaeyoung Ko) — SPDX-License-Identifier: Apache-2.0
# Brevduva 리시버(brv) 설치 — Windows x86_64
#
#   irm https://brevduva.dev/install.ps1 | iex
#
# 하는 일: GitHub Releases 최신 버전에서 바이너리를 받아 SHA256 검증 후
# %USERPROFILE%\.local\bin\brv.exe 로 설치하고(BRV_INSTALL_DIR로 변경 가능),
# 그 경로가 사용자 PATH에 없으면 추가한다(BRV_NO_MODIFY_PATH=1 로 거부 가능). 그 외에는 아무것도 건드리지 않는다.
#
# 진행 표시 (2026-09-05, 사용자 지적 "텍스트만 나와서 설치 중에 멈춘 것처럼 보인다"): Write-Progress
# 막대에 단계 진척률 + 지금 하는 일을, 다운로드는 하위 막대에 받은 양/전체와 초당 속도를 보인다.
# 콘솔이 아닌 호스트에서는 Write-Progress가 조용히 무시되고 결과 메시지만 남는다.
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
# 연결까지 한 줄로 (2026-09-04, 온보딩 재설계 3): `irm … | iex`는 인자를 못 받으므로 env로 —
#   $env:BRV_SERVER='https://…'; $env:BRV_ENROLL='brvenr_…'; irm https://brevduva.dev/install.ps1 | iex
# BRV_INIT_ARGS에 "--unattended" / "--attended-only" / "--runner codex"를 더 줄 수 있다.
if ($env:BRV_SERVER) { $server = $env:BRV_SERVER } else { $server = "https://api.brevduva.dev" }
$enroll = $env:BRV_ENROLL

$repo = "froiibn/brevduva"
$base = "https://github.com/$repo/releases/latest/download"
$asset = "brv-x86_64-pc-windows-msvc.zip"

if ($env:BRV_INSTALL_DIR) { $dest = $env:BRV_INSTALL_DIR } else { $dest = Join-Path $env:USERPROFILE ".local\bin" }
$tmp = Join-Path $env:TEMP ("brv-install-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Force $tmp | Out-Null

# ---- 진행 표시 ----
$script:steps = 7
$script:step = 0
function Step([string]$text) {
    $script:step++
    $pct = [int](($script:step - 1) * 100 / $script:steps)
    Write-Progress -Id 1 -Activity "Installing brv" -Status $text -PercentComplete $pct
}
function Rate([double]$bps) {
    if ($bps -ge 1MB) { return ("{0:N1} MB/s" -f ($bps / 1MB)) }
    return ("{0:N0} KB/s" -f ($bps / 1KB))
}
# 다운로드 — 응답 스트림을 64KB씩 받아 쓰며 0.2초마다 받은 양/전체·초당 속도를 하위 막대에 그린다.
# 전체 크기는 최종 응답의 Content-Length(리다이렉트는 자동 추적). 모르면 받은 양과 속도만.
function Download([string]$url, [string]$out, [string]$label) {
    $req = [Net.HttpWebRequest]::Create($url)
    $req.UserAgent = "brevduva-install"
    $req.AllowAutoRedirect = $true
    $resp = $req.GetResponse()
    $in = $null; $fs = $null
    try {
        $total = [long]$resp.ContentLength   # 모르면 -1
        $in = $resp.GetResponseStream()
        $fs = [IO.File]::Create($out)
        $buf = New-Object byte[] 65536
        $have = [long]0
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $lastTick = [long]0; $lastBytes = [long]0; $speed = [double]0; $nextDraw = [long]0
        while (($n = $in.Read($buf, 0, $buf.Length)) -gt 0) {
            $fs.Write($buf, 0, $n)
            $have += $n
            $ms = $sw.ElapsedMilliseconds
            if ($ms -ge $nextDraw) {
                if ($ms - $lastTick -ge 1000) {
                    $speed = ($have - $lastBytes) * 1000.0 / ($ms - $lastTick)
                    $lastTick = $ms; $lastBytes = $have
                }
                if ($total -gt 0) {
                    $status = "{0:N1} / {1:N1} MB  {2}" -f ($have / 1MB), ($total / 1MB), (Rate $speed)
                    $pct = [int]($have * 100 / $total)
                } else {
                    $status = "{0:N1} MB  {1}" -f ($have / 1MB), (Rate $speed)
                    $pct = -1
                }
                Write-Progress -Id 2 -ParentId 1 -Activity "downloading $label" -Status $status -PercentComplete $pct
                $nextDraw = $ms + 200
            }
        }
        $elapsed = [Math]::Max($sw.ElapsedMilliseconds, 1)
        Write-Progress -Id 2 -ParentId 1 -Activity "downloading $label" -Completed
        return @{ bytes = $have; avg = ($have * 1000.0 / $elapsed) }
    }
    finally {
        if ($fs) { $fs.Dispose() }
        if ($in) { $in.Dispose() }
        $resp.Close()
    }
}

try {
    Step "downloading $asset"
    $got = Download "$base/$asset" "$tmp\$asset" $asset
    Write-Host ("downloaded: {0} ({1:N1} MB, {2} avg)" -f $asset, ($got.bytes / 1MB), (Rate $got.avg))
    Step "downloading SHA256SUMS"
    Download "$base/SHA256SUMS" "$tmp\SHA256SUMS" "SHA256SUMS" | Out-Null

    Step "verifying checksum"
    $line = Select-String -Path "$tmp\SHA256SUMS" -Pattern ([regex]::Escape($asset)) | Select-Object -First 1
    if (-not $line) { throw "no entry for $asset in SHA256SUMS" }
    $expected = ($line.Line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash "$tmp\$asset" -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "checksum mismatch — the download is corrupted or was tampered with" }
    Write-Host "checksum verified"

    Step "installing to $dest"
    Expand-Archive "$tmp\$asset" -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force $dest | Out-Null
    $target = Join-Path $dest "brv.exe"
    # 실행 중인 데몬이 잠근 파일은 덮어쓸 수 없다 (2026-09-03) — 이름을 바꿔 비켜 두고
    # (실행 중에도 허용) 새 파일을 놓는다. 비켜 둔 옛 파일은 다음 설치 때 지운다
    if (Test-Path $target) {
        $old = "$target.old"
        if (Test-Path $old) { Remove-Item $old -Force -ErrorAction SilentlyContinue }
        if (Test-Path $old) { $old = "$target.old." + [guid]::NewGuid().ToString("n") }
        Move-Item $target $old -Force
    }
    Copy-Item "$tmp\brv.exe" $target -Force
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$exe = Join-Path $dest "brv.exe"
Write-Host "installed: $exe — $(& $exe --version)"

Step "updating PATH"
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ';') -notcontains $dest) {
    if ($env:BRV_NO_MODIFY_PATH -eq "1") {
        Write-Host "note: $dest is not on your PATH (BRV_NO_MODIFY_PATH=1, left unchanged)"
    } else {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
        Write-Host "added $dest to your user PATH — new terminals pick it up"
    }
}

# 갱신이면 돌고 있는 서비스 데몬을 새 바이너리로 재기동 (2026-09-03) — 서비스가 없으면 조용히 지나간다
Step "restarting the daemon if one is registered"
$eap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
& $exe daemon restart 2>$null
$ErrorActionPreference = $eap
Write-Progress -Id 1 -Activity "Installing brv" -Status "done" -PercentComplete 100
Write-Progress -Id 1 -Activity "Installing brv" -Completed
if ($enroll) {
    Write-Host ""
    Write-Host "connecting this machine (brv init) ..."
    $initArgs = @("init", "--server", $server, "--enroll", $enroll)
    if ($env:BRV_INIT_ARGS) { $initArgs += ($env:BRV_INIT_ARGS -split '\s+' | Where-Object { $_ }) }
    # 콘솔에서 실행되므로 brv init의 질문(무인 수신 여부·러너 선택)이 그대로 동작한다
    & $exe @initArgs
} else {
    Write-Host ""
    Write-Host "next — connect this machine to your account:"
    Write-Host "  1) https://brevduva.dev dashboard → Connect agents → Connect → copy the one-line command"
    Write-Host "  2) paste it here (it runs: brv init --server $server --enroll <code>)"
}
