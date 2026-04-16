$exePath = 'C:\Users\tyama\CineMantis\apps\desktop\src-tauri\target\release\cinemantis.exe'
$ws  = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut("$env:USERPROFILE\Desktop\CineMantis.lnk")
$lnk.TargetPath       = $exePath
$lnk.WorkingDirectory = Split-Path $exePath
$lnk.Description      = "CineMantis - Video Library Manager"
$lnk.IconLocation     = "$exePath, 0"
$lnk.Save()
Write-Host "Shortcut created: $env:USERPROFILE\Desktop\CineMantis.lnk"
Test-Path "$env:USERPROFILE\Desktop\CineMantis.lnk"
