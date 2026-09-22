//! 照合前入力（pre-match input）の保存ヘルパー。
//!
//! files.original_file_name / original_rel_path は scan 時点の値を残し、
//! TMDB 適用や NAS 整理で書き換わる file_name / file_path とは分けて保持する。
//! 一度値が入った行は db.rs のトリガーで上書きを禁止している。

/// `file_path` を source root からの相対パス（区切りは `/`）に変換する。
/// root の外にあるパスや root 自身は None を返し、絶対パスは決して返さない。
/// Windows のドライブ名・ASCII の大文字小文字差は同一視する。
pub fn relative_to_root(root_path: &str, file_path: &str) -> Option<String> {
    let root = root_path.replace('\\', "/");
    let root = root.trim_end_matches('/');
    let file = file_path.replace('\\', "/");
    if root.is_empty() || file.len() <= root.len() || !file.is_char_boundary(root.len()) {
        return None;
    }
    let (head, rest) = file.split_at(root.len());
    if !head.eq_ignore_ascii_case(root) || !rest.starts_with('/') {
        return None;
    }
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() || is_absolute_like(rest) {
        return None;
    }
    Some(rest.to_string())
}

fn is_absolute_like(path: &str) -> bool {
    path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_path_under_root_becomes_relative() {
        assert_eq!(
            relative_to_root(r"\\nas\movies", r"\\nas\movies\洋画\あ\Alien (1979).mkv"),
            Some("洋画/あ/Alien (1979).mkv".to_string())
        );
        assert_eq!(
            relative_to_root(r"D:\Movies\", r"d:\movies\Alien.1979.mkv"),
            Some("Alien.1979.mkv".to_string())
        );
    }

    #[test]
    fn paths_outside_root_are_rejected() {
        assert_eq!(relative_to_root(r"D:\Movies", r"D:\MoviesOld\a.mkv"), None);
        assert_eq!(relative_to_root(r"D:\Movies", r"E:\Movies\a.mkv"), None);
        assert_eq!(relative_to_root(r"D:\Movies", r"D:\Movies"), None);
        assert_eq!(relative_to_root("", r"D:\Movies\a.mkv"), None);
    }

    #[test]
    fn result_is_never_absolute() {
        for (root, file) in [
            (r"D:\Movies", r"D:\Movies\a.mkv"),
            ("/mnt/nas", "/mnt/nas/sub/a.mkv"),
            (r"\\nas\share", r"\\nas\share\x\y.mkv"),
        ] {
            let rel = relative_to_root(root, file).unwrap();
            assert!(!rel.starts_with('/') && !rel.contains(':'), "{rel}");
        }
    }
}
