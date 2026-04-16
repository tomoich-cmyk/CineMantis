/// プラットフォーム固有のユーティリティコマンド

/// デスクトップにショートカット（.lnk）を作成する（Windows 専用）
///
/// PowerShell の WScript.Shell COM オブジェクトを使用して
/// %USERPROFILE%\Desktop\CineMantis.lnk を生成する。
/// exe のパスはランタイムに取得するため、どのインストール先でも機能する。
#[tauri::command]
pub fn create_desktop_shortcut() -> Result<(), String> {
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe_path.to_string_lossy();

    // exe が格納されているフォルダを作業ディレクトリにする
    let work_dir = exe_path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    // PowerShell スクリプトで .lnk を生成
    // パス内のダブルクォートをエスケープして安全に埋め込む
    let exe_escaped  = exe_str.replace('"', r#"`""#);
    let work_escaped = work_dir.replace('"', r#"`""#);

    let ps_script = format!(
        r#"
$ws  = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut("$env:USERPROFILE\Desktop\CineMantis.lnk")
$lnk.TargetPath       = "{exe}"
$lnk.WorkingDirectory = "{work}"
$lnk.Description      = "CineMantis - Video Library Manager"
$lnk.IconLocation     = "{exe}, 0"
$lnk.Save()
"#,
        exe  = exe_escaped,
        work = work_escaped,
    );

    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;

    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_script]);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    let output = cmd
        .output()
        .map_err(|e| format!("PowerShell の起動に失敗しました: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ショートカット作成に失敗しました: {stderr}"));
    }

    Ok(())
}
