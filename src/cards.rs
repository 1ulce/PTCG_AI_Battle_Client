//! 軽量カード照会層 ([`CardFacts`])。
//!
//! bot が判断に使うカード事実は実質 2 つだけ:
//!
//! 1. `is_ex(slug)` — その slug が「ルールを持つ (ex 等、サイド 2 枚)」ポケモンか
//! 2. `attack_index(slug, name)` — ワザ名 → ワザ index (YAML `position` 順)
//!
//! 本体は `CardRegistry` (POL DSL + master 統合) からこれを引くが、公開クライアントは
//! engine 非依存なので pokemon-card-data の YAML (`{slug}.yml`) から直接読む。
//!
//! - prize_value: `meta_tags` から算出 (`mega`→3 / `ex`→2 / その他→1)。本体
//!   `card_loader::parse_meta_tags` と同じ規則。`is_ex` は `prize_value >= 2`。
//! - attack index: `attacks[].position` 順のワザ名一覧での位置。POL DSL のワザ順は master の
//!   position と 1:1 対応するため、これがエンジンの `attack_index` と一致する。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// カード事実の参照テーブル (slug キー)。
#[derive(Debug, Clone, Default)]
pub struct CardFacts {
    by_slug: HashMap<String, CardFact>,
}

#[derive(Debug, Clone)]
struct CardFact {
    prize_value: u8,
    /// ワザ名 (master の `position` 昇順 = エンジンの attack_index 基準)。
    attack_names: Vec<String>,
}

/// [`CardFacts::load_from_dir`] のエラー。
#[derive(Debug)]
pub enum CardFactsError {
    Io(std::io::Error),
    Yaml {
        path: String,
        source: serde_yaml::Error,
    },
}

impl std::fmt::Display for CardFactsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "card data io: {e}"),
            Self::Yaml { path, source } => write!(f, "card data parse {path}: {source}"),
        }
    }
}

impl std::error::Error for CardFactsError {}

impl From<std::io::Error> for CardFactsError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl CardFacts {
    /// 空のテーブル (未知 slug は `is_ex=false` / `attack_index=None`)。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// pokemon-card-data の `cards/` ディレクトリ (`{slug}.yml`) を読み込む。
    ///
    /// `*.yml` / `*.yaml` のみ対象。bot が必要とする `slug` / `meta_tags` / `attacks` だけを
    /// 取り出す (他フィールドは無視)。
    ///
    /// # Errors
    /// ディレクトリの読み込み失敗 / いずれかの YAML のパース失敗。
    pub fn load_from_dir<P: AsRef<Path>>(dir: P) -> Result<Self, CardFactsError> {
        let mut by_slug = HashMap::new();
        for entry in std::fs::read_dir(dir.as_ref())? {
            let entry = entry?;
            let path = entry.path();
            let is_yaml = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e == "yml" || e == "yaml");
            if !is_yaml {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            let card: YamlCard =
                serde_yaml::from_str(&text).map_err(|source| CardFactsError::Yaml {
                    path: path.display().to_string(),
                    source,
                })?;
            by_slug.insert(card.slug.clone(), CardFact::from_yaml(card));
        }
        Ok(Self { by_slug })
    }

    /// slug が「ルールを持つ (ex 等、サイド 2 枚)」ポケモンか。未知 slug は `false`。
    #[must_use]
    pub fn is_ex(&self, slug: &str) -> bool {
        self.by_slug.get(slug).is_some_and(|c| c.prize_value >= 2)
    }

    /// ワザ名 → ワザ index (master `position` 順)。未知 slug / 未知ワザは `None`。
    #[must_use]
    pub fn attack_index(&self, slug: &str, name: &str) -> Option<u8> {
        let card = self.by_slug.get(slug)?;
        let pos = card.attack_names.iter().position(|n| n == name)?;
        u8::try_from(pos).ok()
    }

    /// テスト / 手組み用: 1 枚分の事実を登録する (builder スタイル)。
    #[must_use]
    pub fn with_card(mut self, slug: &str, prize_value: u8, attack_names: &[&str]) -> Self {
        self.by_slug.insert(
            slug.to_string(),
            CardFact {
                prize_value,
                attack_names: attack_names.iter().map(|s| (*s).to_string()).collect(),
            },
        );
        self
    }
}

impl CardFact {
    fn from_yaml(card: YamlCard) -> Self {
        let prize_value = prize_value_from_meta(&card.meta_tags);
        let mut attacks = card.attacks;
        attacks.sort_by_key(|a| a.position);
        let attack_names = attacks
            .into_iter()
            .filter_map(|a| a.name.ja)
            .collect::<Vec<_>>();
        Self {
            prize_value,
            attack_names,
        }
    }
}

/// `meta_tags` → prize_value。本体 `card_loader::parse_meta_tags` と同規則
/// (`mega`→3 / `ex`→2 / その他→1)。
fn prize_value_from_meta(tags: &[String]) -> u8 {
    if tags.iter().any(|t| t == "mega") {
        3
    } else if tags.iter().any(|t| t == "ex") {
        2
    } else {
        1
    }
}

// ============================================================================
// 入力 YAML schema (pokemon-card-data v1、bot 必要分のみ)
// ============================================================================

#[derive(Debug, Deserialize)]
struct YamlCard {
    slug: String,
    #[serde(default)]
    meta_tags: Vec<String>,
    #[serde(default)]
    attacks: Vec<YamlAttack>,
}

#[derive(Debug, Deserialize)]
struct YamlAttack {
    #[serde(default)]
    position: u8,
    #[serde(default)]
    name: YamlName,
}

#[derive(Debug, Default, Deserialize)]
struct YamlName {
    #[serde(default)]
    ja: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_tags_to_prize() {
        assert_eq!(prize_value_from_meta(&["ex".to_string()]), 2);
        assert_eq!(prize_value_from_meta(&["mega".to_string()]), 3);
        assert_eq!(prize_value_from_meta(&[]), 1);
    }

    #[test]
    fn parses_dragapult_ex_yaml() {
        let yaml = r"
slug: dragapult-ex
card_type: Pokémon
meta_tags:
- ex
attacks:
- position: 0
  name:
    ja: ジェットヘッド
- position: 1
  name:
    ja: ファントムダイブ
";
        let card: YamlCard = serde_yaml::from_str(yaml).unwrap();
        let facts = CardFacts {
            by_slug: std::iter::once(("dragapult-ex".to_string(), CardFact::from_yaml(card)))
                .collect(),
        };
        assert!(facts.is_ex("dragapult-ex"));
        assert_eq!(
            facts.attack_index("dragapult-ex", "ジェットヘッド"),
            Some(0)
        );
        assert_eq!(
            facts.attack_index("dragapult-ex", "ファントムダイブ"),
            Some(1)
        );
        assert_eq!(facts.attack_index("dragapult-ex", "未知"), None);
    }

    #[test]
    fn unknown_slug_is_not_ex() {
        let facts = CardFacts::new();
        assert!(!facts.is_ex("whatever"));
        assert_eq!(facts.attack_index("whatever", "x"), None);
    }

    #[test]
    fn with_card_builder() {
        let facts = CardFacts::new().with_card("opp-ex", 2, &["a", "b"]);
        assert!(facts.is_ex("opp-ex"));
        assert_eq!(facts.attack_index("opp-ex", "b"), Some(1));
    }
}
