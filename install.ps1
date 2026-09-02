# Copyright 2026 SEIZIA (Jaeyoung Ko) — SPDX-License-Identifier: Apache-2.0
# Brevduva 리시버(brv) 설치 — Windows x86_64
#
#   irm https://brevduva.dev/install.ps1 | iex
#
# 하는 일: GitHub Releases 최신 버전에서 바이너리를 받아 SHA256 검증 후
# %USERPROFILE%\.local\bin\brv.exe 로 설치하고(BRV_INSTALL_DIR로 변경 가능),
# 그 경로가 사용자 PATH에 없으면 추가한다. 그 외에는 아무것도 건드리지 않는다.
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = "froiibn/brevduva"
$base = "https://github.com/$repo/releases/latest/download"
$asset = "brv-x86_64-pc-windows-msvc.zip"

if ($env:BRV_INSTALL_DIR) { $dest = $env:BRV_INSTALL_DIR } else { $dest = Join-Path $env:USERPROFILE ".local\bin" }
$tmp = Join-Path $env:TEMP ("brv-install-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Force $tmp | Out-Null

try {
    Write-Host "downloading: $base/$asset"
    Invoke-WebRequest "$base/$asset" -OutFile "$tmp\$asset" -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing

    $line = Select-String -Path "$tmp\SHA256SUMS" -Pattern ([regex]::Escape($asset)) | Select-Object -First 1
    if (-not $line) { throw "no entry for $asset in SHA256SUMS" }
    $expected = ($line.Line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash "$tmp\$asset" -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "checksum mismatch — the download is corrupted or was tampered with" }
    Write-Host "checksum verified"

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

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ';') -notcontains $dest) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
    Write-Host "added $dest to your user PATH — new terminals pick it up"
}

# 갱신이면 돌고 있는 서비스 데몬을 새 바이너리로 재기동 (2026-09-03) — 서비스가 없으면 조용히 지나간다
$eap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
& $exe daemon restart 2>$null
$ErrorActionPreference = $eap

Write-Host ""
Write-Host "next — connect this machine to your account:"
Write-Host "  1) https://brevduva.dev dashboard → Connect a machine → issue an enroll code (starts with brvenr_)"
Write-Host "  2) brv init --server https://api.brevduva.dev --enroll <code>"
