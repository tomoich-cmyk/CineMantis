use serde::Serialize;

/// works.match_source の取りうる値。
/// 「今ある照合が、どの経路で付いたか」を表す（誰が確定したかの履歴は
/// metadata_match_labels、なぜレビューされたかは metadata_review_tasks に持つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchSource {
    /// rules-safe が AUTO と判定して自動適用した
    RulesSafeAuto,
    /// 人が候補を選んだ / TMDb ID を直接指定した
    Manual,
    /// 021 より前に付いていた照合（経路が分からない）
    Legacy,
}

impl MatchSource {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchSource::RulesSafeAuto => "rules_safe_auto",
            MatchSource::Manual => "manual",
            MatchSource::Legacy => "legacy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_match_the_documented_strings() {
        assert_eq!(MatchSource::RulesSafeAuto.as_str(), "rules_safe_auto");
        assert_eq!(MatchSource::Manual.as_str(), "manual");
        assert_eq!(MatchSource::Legacy.as_str(), "legacy");
    }
}
