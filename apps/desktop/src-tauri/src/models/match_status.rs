use serde::Serialize;

/// works.match_status の取りうる値（001_initial.sql の CHECK 制約と一致させる）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchStatus {
    /// 未照合（自動照合で候補なし = UNRESOLVED を含む）
    Unmatched,
    /// レビュー待ち（候補はあるが自動確定できない = REVIEW）
    Pending,
    /// 照合済み
    Matched,
    /// 固定。変更には明示的な unlock が必要
    Locked,
}

impl MatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchStatus::Unmatched => "unmatched",
            MatchStatus::Pending => "pending",
            MatchStatus::Matched => "matched",
            MatchStatus::Locked => "locked",
        }
    }

    /// 外部入力（Tauri コマンド引数など）を検証して変換する
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "unmatched" => Ok(MatchStatus::Unmatched),
            "pending" => Ok(MatchStatus::Pending),
            "matched" => Ok(MatchStatus::Matched),
            "locked" => Ok(MatchStatus::Locked),
            other => Err(format!("不正な照合状態です: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_only_db_values() {
        for status in [
            MatchStatus::Unmatched,
            MatchStatus::Pending,
            MatchStatus::Matched,
            MatchStatus::Locked,
        ] {
            assert_eq!(MatchStatus::parse(status.as_str()), Ok(status));
        }
        assert!(MatchStatus::parse("auto").is_err());
        assert!(MatchStatus::parse("manual").is_err());
        assert!(MatchStatus::parse("").is_err());
        assert!(MatchStatus::parse("Matched").is_err());
    }
}
