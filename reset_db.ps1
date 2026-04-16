# 古いDBを削除（スキーマが古いため、再作成させる）
$dbDir = "$env:APPDATA\dev.cinemantis.app"
Write-Host "DB directory: $dbDir"
Get-ChildItem $dbDir -ErrorAction SilentlyContinue | Select-Object Name, Length

$files = @(
    "$dbDir\cinemantis.db",
    "$dbDir\cinemantis.db-shm",
    "$dbDir\cinemantis.db-wal"
)
foreach ($f in $files) {
    if (Test-Path $f) {
        Remove-Item $f -Force
        Write-Host "Deleted: $f"
    }
}
Write-Host "Done. DB will be recreated on next app launch."
