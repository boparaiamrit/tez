# End-to-end smoke test on Windows: install PHP and Redis through tez, serve a site, check PHP and
# the metrics API, then stop. Used by CI and releases.
$ErrorActionPreference = 'Stop'
$bin = if ($args.Count -gt 0) { $args[0] } else { '.\target\release\tez.exe' }

function Show-Logs { Get-ChildItem "$env:USERPROFILE\.tez\logs" -Filter *.log -ErrorAction SilentlyContinue | ForEach-Object { "---- $($_.Name)"; Get-Content $_.FullName -Tail 30 } }

& $bin install --php 8.4 --services redis
if ($LASTEXITCODE -ne 0) { Show-Logs; exit 1 }
$env:Path = "$env:USERPROFILE\.tez\bin;$env:Path"
tez version
php -v

New-Item -ItemType Directory -Force "$env:USERPROFILE\Sites\hello\public" | Out-Null
Set-Content "$env:USERPROFILE\Sites\hello\public\index.php" '<?php echo "tez-ok-" . PHP_VERSION;'
tez start
$st = tez status --json | ConvertFrom-Json
$ok = $false; $body = ''
for ($i = 0; $i -lt 60; $i++) {
  try { $body = (Invoke-WebRequest -UseBasicParsing -Headers @{ Host = 'hello.localhost' } "http://127.0.0.1:$($st.http_port)/").Content } catch { }
  if ($body -like 'tez-ok-8.4.*') { $ok = $true; break }
  Start-Sleep -Seconds 1
}
"site says: $body"
if (-not $ok) { Show-Logs; exit 1 }

$m = Invoke-RestMethod 'http://127.0.0.1:7979/api/metrics?range=15m'
if (-not ($m.machine.total -gt 0)) { 'metrics missing'; exit 1 }
'metrics: ok'
tez stop
