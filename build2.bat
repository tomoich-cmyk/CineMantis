call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if errorlevel 1 (
    echo vcvars failed
    exit /b 1
)
set PATH=C:\Users\tyama\.cargo\bin;C:\Users\tyama\AppData\Local\pnpm\.tools\pnpm\9.0.0_tmp_3864_0\bin;%PATH%
echo LIB=%LIB%
where link.exe
where pnpm
cd /d C:\Users\tyama\CineMantis
pnpm --filter desktop tauri build
