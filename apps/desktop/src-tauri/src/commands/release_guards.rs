//! リリース設定の回帰防止テスト（version / devtools）

fn field<'a>(toml: &'a str, key: &str) -> Option<&'a str> {
    toml.lines()
        .find_map(|l| l.trim().strip_prefix(key)?.trim_start().strip_prefix('='))
        .map(|v| v.trim().trim_matches('"'))
}

/// release では tauri の devtools feature を有効にしない
#[test]
fn devtools_feature_not_forced_in_manifest() {
    let m = include_str!("../../Cargo.toml");
    assert!(!m.contains("\"devtools\""), "tauri devtools feature must not be enabled");
}

/// Cargo.toml の package.version が唯一の source。tauri.conf.json は version を持たない
#[test]
fn tauri_conf_has_no_version() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    assert!(v.get("version").is_none());
}

/// アプリ version は 1.0.0（変更時はこのテストも意図的に更新する）
#[test]
fn app_version_is_cargo_version() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "1.0.0");
    assert_eq!(field(include_str!("../../Cargo.toml"), "version"), Some("1.0.0"));
}

/// package.json は version を持たない（Cargo.toml と二重管理しない）
#[test]
fn package_json_has_no_version() {
    for s in [
        include_str!("../../../package.json"),
        include_str!("../../../../../package.json"),
    ] {
        let v: serde_json::Value = serde_json::from_str(s).unwrap();
        assert!(v.get("version").is_none());
    }
}

#[test]
fn settings_screen_uses_runtime_app_version() {
    let s = include_str!("../../../src/components/settings/SettingsScreen.tsx");
    assert!(s.contains("getVersion"));
    assert!(s.contains("<span>{appVersion}</span>"));
    assert!(!s.contains("<span>0.1.0</span>"));
}

/// UI source tree (src/**/*.ts{,x}) must not hardcode the old app version.
#[test]
fn ui_source_has_no_hardcoded_old_app_version() {
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, hits);
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("ts" | "tsx")) {
                let s = std::fs::read_to_string(&p).unwrap();
                if s.contains("0.1.0") {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    let mut hits = vec![];
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src"), &mut hits);
    assert!(hits.is_empty(), "hardcoded 0.1.0 in UI source: {hits:?}");
}

/// WebView2 DevTools: tauri-runtime-wry passes `devtools.unwrap_or(true)` to wry (SetAreDevToolsEnabled),
/// so release builds need the window config to say false explicitly (the Cargo feature alone is not enough).
#[test]
fn main_window_devtools_explicitly_disabled() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let wins = v["app"]["windows"].as_array().unwrap();
    assert!(!wins.is_empty());
    for w in wins {
        assert_eq!(w["devtools"], serde_json::Value::Bool(false), "window devtools must be false");
    }
}

/// NSIS: custom template (upstream tauri-cli 2.10.1 minus delete-app-data) is wired in,
/// the old hook approach is gone, and the template has no delete-app-data checkbox / recursive delete.
#[test]
fn nsis_uninstall_preserves_app_data() {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let nsis = &v["bundle"]["windows"]["nsis"];
    assert_eq!(nsis["template"].as_str(), Some("installer-template.nsi"));
    assert!(nsis.get("installerHooks").is_none());
    assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("installer-hooks.nsh").exists());
    assert_eq!(v["identifier"].as_str(), Some("dev.cinemantis.app"));

    let t = include_str!("../../installer-template.nsi").to_lowercase();
    for banned in ["deleteappdata", "checkbox"] {
        assert!(!t.contains(banned), "installer template must not contain `{banned}`");
    }
    // recursive directory delete (the `/r` flag as its own token; `/REBOOTOK` is fine)
    for line in t.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        assert!(
            !w.windows(2).any(|p| p[0] == "rmdir" && p[1] == "/r"),
            "recursive delete in installer template: {line}"
        );
    }
    assert!(t.contains("section uninstall"));
}
