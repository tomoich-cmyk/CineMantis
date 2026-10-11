//! PR4-PF0G3: reviewer 実行ファイルの本体（`examples/pf0g3_reviewer.rs` は薄い main）。
//!
//! 守ること:
//! - 設定は **環境変数だけ**（引数に secret を置かない = process 一覧に出ない）。必須の欠落・形式不正は起動せず Err（fail closed）。
//! - API key は専用の変数（本体アプリの設定・production DB とは共有しない）。値は Debug・log・Err・evidence のどこにも出さず、presence だけを報告する。
//! - 通信は 127.0.0.1 の loopback 受付のみ。TMDB へ出るのは `Production` モードのときだけ（テストは `Loopback` のみ使う）。
//! - 本体アプリの稼働は `SystemProcessLister` で起動前と全 command で確認する（一覧が取れなければ「稼働中」）。

use std::path::{Path, PathBuf};

use super::pr4_blind_gt::{Artifact, BlindError, Store};
use super::pr4_pf0g2::{ProcessLister, ProcessProbe};
use super::pr4_pf0g2_server::{system_clock, Api, HttpTmdb, Server, TmdbTransport};

pub use super::pr4_pf0g2::SystemProcessLister;

type R<T> = Result<T, BlindError>;

pub const ENV_REVIEWER_DIR: &str = "CM_PF0G3_REVIEWER_DIR";
pub const ENV_LEDGER_DIR: &str = "CM_PF0G3_LEDGER_DIR";
pub const ENV_CHECKPOINT_DIR: &str = "CM_PF0G3_CHECKPOINT_DIR";
pub const ENV_CACHE_DIR: &str = "CM_PF0G3_CACHE_DIR";
pub const ENV_PORT: &str = "CM_PF0G3_PORT";
pub const ENV_TMDB_API_KEY: &str = "CM_PF0G3_TMDB_API_KEY";
/// テスト専用: `127.0.0.1:<port>` を指すと TMDB の代わりにそこへ出る。未設定なら production TMDB
pub const ENV_TMDB_LOOPBACK: &str = "CM_PF0G3_TMDB_LOOPBACK";
pub const ALL_ENV: [&str; 7] = [ENV_REVIEWER_DIR, ENV_LEDGER_DIR, ENV_CHECKPOINT_DIR, ENV_CACHE_DIR, ENV_PORT, ENV_TMDB_API_KEY, ENV_TMDB_LOOPBACK];

/// 値を Debug に出さない secret
#[derive(Clone)]
pub struct Secret(String);
impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<redacted>")
    }
}

#[derive(Debug, Clone)]
pub enum TmdbMode {
    Production,
    Loopback(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub reviewer_dir: PathBuf,
    pub ledger_dir: PathBuf,
    pub checkpoint_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub port: u16,
    pub tmdb: TmdbMode,
    pub api_key: Secret,
}

fn invalid<T>(m: impl Into<String>) -> R<T> {
    Err(BlindError::Invalid(m.into()))
}

fn dir_var(get: &dyn Fn(&str) -> Option<String>, name: &str) -> R<PathBuf> {
    let Some(v) = get(name) else { return invalid(format!("{name} が未設定です")) };
    if v.trim().is_empty() || v.contains('\0') {
        return invalid(format!("{name} が空または不正です"));
    }
    let p = PathBuf::from(v.trim());
    if !p.is_absolute() {
        return invalid(format!("{name} は絶対 path にしてください"));
    }
    Ok(p)
}

fn nested(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
    let (x, y) = (norm(a), norm(b));
    x.starts_with(&y) || y.starts_with(&x)
}

impl RuntimeConfig {
    /// 環境変数（`get`）から作る。必須の欠落・形式不正は Err。Err の文言に値は入れない
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> R<RuntimeConfig> {
        let reviewer_dir = dir_var(get, ENV_REVIEWER_DIR)?;
        let ledger_dir = dir_var(get, ENV_LEDGER_DIR)?;
        let checkpoint_dir = dir_var(get, ENV_CHECKPOINT_DIR)?;
        let cache_dir = dir_var(get, ENV_CACHE_DIR)?;
        let dirs = [(&reviewer_dir, ENV_REVIEWER_DIR), (&ledger_dir, ENV_LEDGER_DIR), (&checkpoint_dir, ENV_CHECKPOINT_DIR), (&cache_dir, ENV_CACHE_DIR)];
        for i in 0..dirs.len() {
            for j in i + 1..dirs.len() {
                if nested(dirs[i].0, dirs[j].0) {
                    return invalid(format!("{} と {} は同一または入れ子にできません（reviewer 側と sealed / audit 側の分離）", dirs[i].1, dirs[j].1));
                }
            }
        }
        let port = match get(ENV_PORT) {
            None => 0,
            Some(v) => match v.trim().parse::<u16>() {
                Ok(p) => p,
                Err(_) => return invalid(format!("{ENV_PORT} は 0..65535 の整数にしてください")),
            },
        };
        let Some(key) = get(ENV_TMDB_API_KEY) else { return invalid(format!("{ENV_TMDB_API_KEY} が未設定です")) };
        let key = key.trim().to_string();
        if key.is_empty() || key.len() > 512 || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            return invalid(format!("{ENV_TMDB_API_KEY} の形式が不正です"));
        }
        let tmdb = match get(ENV_TMDB_LOOPBACK) {
            None => TmdbMode::Production,
            Some(a) => {
                let ok = a.strip_prefix("127.0.0.1:").map(|p| p.parse::<u16>().is_ok()).unwrap_or(false);
                if !ok {
                    return invalid(format!("{ENV_TMDB_LOOPBACK} は 127.0.0.1:<port> だけです"));
                }
                TmdbMode::Loopback(a)
            }
        };
        Ok(RuntimeConfig { reviewer_dir, ledger_dir, checkpoint_dir, cache_dir, port, tmdb, api_key: Secret(key) })
    }

    pub fn from_env() -> R<RuntimeConfig> {
        Self::from_lookup(&|n| std::env::var(n).ok())
    }

    fn transport(&self) -> R<Box<dyn TmdbTransport>> {
        Ok(match &self.tmdb {
            TmdbMode::Production => Box::new(HttpTmdb::production(self.api_key.expose())?),
            TmdbMode::Loopback(a) => Box::new(HttpTmdb::loopback(a, self.api_key.expose(), std::time::Duration::from_secs(10))?),
        })
    }
}

/// 環境変数の有無だけ（値・長さは出さない）。evidence 用
pub fn env_presence(get: &dyn Fn(&str) -> Option<String>) -> Vec<(String, bool)> {
    ALL_ENV.iter().map(|n| (n.to_string(), get(n).map(|v| !v.trim().is_empty()).unwrap_or(false))).collect()
}

/// reviewer を起動する。本体アプリが動いていれば（または一覧が取れなければ）起動せず `MainAppRunning`
pub fn launch<L>(cfg: &RuntimeConfig, lister: L) -> R<Server>
where
    L: ProcessLister + Clone + Send + 'static,
{
    // Store / Api の前に、設定だけで確定できる拒否を先に行う（ledger に触れない）
    if (ProcessProbe { lister: lister.clone() }).main_app_running_checked() {
        return Err(BlindError::MainAppRunning);
    }
    let port = cfg.port;
    let cfg = cfg.clone();
    Server::start(
        move || {
            let probe = ProcessProbe { lister: lister.clone() };
            let art = Artifact::load(&cfg.reviewer_dir)?;
            let store = Store::open(&cfg.ledger_dir, &cfg.checkpoint_dir, art, Box::new(ProcessProbe { lister }))?;
            Api::attach(store, &cfg.ledger_dir, &cfg.cache_dir, cfg.transport()?, system_clock(), &probe)
        },
        port,
    )
}
