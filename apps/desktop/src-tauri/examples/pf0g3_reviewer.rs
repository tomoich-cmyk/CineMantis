//! PR4-PF0G3 reviewer 実行ファイル。`cargo run --example pf0g3_reviewer`（製品の bundle には入らない）。
//! 設定は環境変数のみ（`CM_PF0G3_*`、一覧は `pr4_pf0g3_reviewer::ALL_ENV`）。値は表示しない。
use cinemantis_lib::pf0g3_reviewer::{env_presence, launch, RuntimeConfig, SystemProcessLister};

fn main() {
    let cfg = match RuntimeConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("reviewer: 起動できません: {e}");
            for (name, present) in env_presence(&|n| std::env::var(n).ok()) {
                eprintln!("  {name}: {}", if present { "set" } else { "unset" });
            }
            std::process::exit(2);
        }
    };
    match launch(&cfg, SystemProcessLister) {
        Ok(srv) => {
            // token は起動ごとのもの（API key ではない）
            println!("reviewer: http://{}/?t={}", srv.addr, srv.token);
            println!("reviewer: Enter で終了します");
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            srv.stop();
        }
        Err(e) => {
            eprintln!("reviewer: 起動できません: {e}");
            std::process::exit(3);
        }
    }
}
