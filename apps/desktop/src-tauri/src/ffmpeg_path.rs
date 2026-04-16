/// ffmpeg / ffprobe の実行ファイルパスを動的に解決するヘルパー
///
/// Tauri アプリをデスクトップショートカット経由で起動した場合、
/// WinGet でインストールされた ffmpeg が PATH に含まれないことがある。
/// `where.exe` による検索と WinGet パッケージディレクトリのスキャンで対応する。

pub fn find_ffmpeg() -> String {
    find_media_tool("ffmpeg")
}

pub fn find_ffprobe() -> String {
    find_media_tool("ffprobe")
}

fn find_media_tool(name: &str) -> String {
    // 1. where.exe でシステム PATH から検索
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        if let Ok(out) = std::process::Command::new("where")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = s.lines().next() {
                    let trimmed = line.trim().to_string();
                    if std::path::Path::new(&trimmed).exists() {
                        return trimmed;
                    }
                }
            }
        }

        // 2. WinGet Packages ディレクトリをスキャン
        if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
            let packages = std::path::PathBuf::from(local_app)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages");
            if packages.exists() {
                let exe_name = format!("{name}.exe");
                if let Some(found) = scan_for_exe(&packages, &exe_name, 0) {
                    return found;
                }
            }
        }

        // 3. Scoop の shims
        if let Ok(home) = std::env::var("USERPROFILE") {
            let shim = std::path::PathBuf::from(home)
                .join("scoop")
                .join("shims")
                .join(format!("{name}.exe"));
            if shim.exists() {
                return shim.to_string_lossy().to_string();
            }
        }

        // 4. Chocolatey
        let choco = std::path::PathBuf::from(r"C:\ProgramData\chocolatey\bin")
            .join(format!("{name}.exe"));
        if choco.exists() {
            return choco.to_string_lossy().to_string();
        }
    }

    // 最終フォールバック: 名前のみ（PATH に含まれていれば動く）
    name.to_string()
}

/// `dir` 以下を再帰スキャンして `exe_name` を探す（最大深さ 6 まで）
#[cfg(target_os = "windows")]
fn scan_for_exe(dir: &std::path::Path, exe_name: &str, depth: u32) -> Option<String> {
    if depth > 6 {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = scan_for_exe(&p, exe_name, depth + 1) {
                return Some(found);
            }
        } else if p.file_name().and_then(|n| n.to_str()) == Some(exe_name) {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}
