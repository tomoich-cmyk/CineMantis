pub mod audit_run;
pub mod container_tags;
/// 手動作業用のランナー。製品には出ない（test ビルドのときだけ）
#[cfg(test)]
pub mod gt_operator;
pub mod gt_review;
pub mod gt_sampling;
pub mod jev_contract;
pub mod jev_session;
pub mod jev_shadow;
pub mod jev_sidecar;
pub mod jev_state;
pub mod kana_bucket;
pub mod title_parser;
pub mod tmdb_client;
pub mod typesafe_client;
pub mod match_history;
pub mod metadata_matcher;
pub mod poster_store;
pub mod prematch_inputs;
pub mod pr4_diagnostics;
/// PR4-3 の実データ検証 runner。手動専用で、製品には出ない（test ビルドのときだけ）
#[cfg(test)]
pub mod pr4_3v_runner;
/// PR4-PF0A: holdout の件数と所属の commitment（count-only・手動専用）。製品には出ない
#[cfg(test)]
pub mod pr4_pf0a;
/// PR4-PF0B: FINAL パイプラインの予行演習（synthetic のみ・test ビルドだけ）。製品には出ない
#[cfg(test)]
pub mod pr4_pf0b;
/// PR4-PF0D: H2 蓄積の feasibility census（count-only・手動専用）。製品には出ない
#[cfg(test)]
pub mod pr4_pf0d;
pub mod prematch_snapshot;
pub mod reading;
pub mod rules_v1;
