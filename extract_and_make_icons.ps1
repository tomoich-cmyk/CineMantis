Add-Type -AssemblyName System.Drawing

# JSONL から base64 画像データを抽出
$jsonlPath = 'C:\Users\tyama\.claude\projects\C--Users-tyama-CineMantis\eed3c21a-e8f3-4edf-9c0a-91340dc395f0.jsonl'
$lines = Get-Content $jsonlPath -Raw

# 最後の画像データ（ユーザーが今回送った画像）を抽出
$pattern = '"type":"image","source":\{"type":"base64","media_type":"image/png","data":"([^"]+)"'
$matches_all = [regex]::Matches($lines, $pattern)
if ($matches_all.Count -eq 0) {
    Write-Error "画像データが見つかりません"
    exit 1
}
# 最後のマッチ（最新の画像）
$b64 = $matches_all[$matches_all.Count - 1].Groups[1].Value
Write-Host "画像データ取得: $($b64.Length) chars"

# PNG として保存
$pngBytes = [Convert]::FromBase64String($b64)
$srcPath = 'C:\Users\tyama\CineMantis\logo_source.png'
[System.IO.File]::WriteAllBytes($srcPath, $pngBytes)
Write-Host "元画像保存: $srcPath ($($pngBytes.Length) bytes)"

# アイコンディレクトリ
$iconsDir = 'C:\Users\tyama\CineMantis\apps\desktop\src-tauri\icons'
New-Item -ItemType Directory -Force -Path $iconsDir | Out-Null

# 元画像を読み込む
$srcBmp = [System.Drawing.Bitmap]::FromFile($srcPath)
Write-Host "元画像サイズ: $($srcBmp.Width) x $($srcBmp.Height)"

function Resize-Bitmap($src, $size) {
    $dst = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($dst)
    $g.InterpolationMode  = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode      = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode    = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.DrawImage($src, 0, 0, $size, $size)
    $g.Dispose()
    return $dst
}

# PNG サイズ群
$pngSizes = @(
    @{size=32;  name='32x32.png'},
    @{size=128; name='128x128.png'},
    @{size=256; name='128x128@2x.png'},
    @{size=256; name='256x256.png'},
    @{size=32;  name='icon.png'}
)
foreach ($entry in $pngSizes) {
    $bmp = Resize-Bitmap $srcBmp $entry.size
    $bmp.Save("$iconsDir\$($entry.name)", [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "Saved: $($entry.name)"
}

# ICO ファイル生成（16, 32, 48, 256）
function New-IcoFromBitmap($src, $sizes_list) {
    $images = @{}
    foreach ($s in $sizes_list) {
        $bmp = Resize-Bitmap $src $s
        $ms  = New-Object System.IO.MemoryStream
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $images[$s] = $ms.ToArray()
        $ms.Dispose()
        $bmp.Dispose()
    }
    $count    = $sizes_list.Count
    $headerSz = 6 + 16 * $count
    $offset   = $headerSz
    $offsets  = @{}
    foreach ($s in $sizes_list) { $offsets[$s] = $offset; $offset += $images[$s].Length }

    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    $bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$count)
    foreach ($s in $sizes_list) {
        $dim = if ($s -eq 256) { 0 } else { $s }
        $bw.Write([byte]$dim); $bw.Write([byte]$dim)
        $bw.Write([byte]0); $bw.Write([byte]0)
        $bw.Write([uint16]1); $bw.Write([uint16]32)
        $bw.Write([uint32]$images[$s].Length)
        $bw.Write([uint32]$offsets[$s])
    }
    foreach ($s in $sizes_list) { $bw.Write($images[$s]) }
    $bw.Flush()
    return $ms.ToArray()
}

$icoBytes = New-IcoFromBitmap $srcBmp @(16, 32, 48, 256)
[System.IO.File]::WriteAllBytes("$iconsDir\icon.ico", $icoBytes)
Write-Host "Saved: icon.ico ($($icoBytes.Length) bytes)"

$srcBmp.Dispose()
Write-Host "`n完了！"
Get-ChildItem $iconsDir | Select-Object Name, Length
