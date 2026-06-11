//! デッキリスト (slug + 枚数) — 持参デッキ (BYO) として subscribe に載せる。
//!
//! 本体 `engine-core::deck::DeckList` と同じ serde 形。クライアントは resolve/validate
//! しない (サーバが自分の registry で解決・検証する) ので、YAML パースだけ持つ。
//!
//! ## フォーマット (YAML)
//!
//! ```yaml
//! name: Dragapult ex
//! cards:
//!   - { slug: dreepy, count: 4 }
//!   - { slug: fire-energy, count: 4 }
//! ```

use std::path::Path;

use serde::{Deserialize, Serialize};

/// デッキリスト 1 件 (slug + 枚数の宣言)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckList {
    /// 表示名 (任意)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// カード行 (slug + 枚数)。
    pub cards: Vec<DeckEntry>,
}

/// デッキリストの 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckEntry {
    pub slug: String,
    pub count: u8,
}

impl DeckList {
    /// YAML 文字列からパースする。
    ///
    /// # Errors
    /// YAML 構文エラー。
    pub fn from_yaml_str(s: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(s)
    }

    /// ファイル (`*.yaml`/`*.yml`) から読み込む。
    ///
    /// # Errors
    /// I/O エラー / YAML 構文エラー。
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, DeckLoadError> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_yaml_str(&text)?)
    }

    /// 宣言された総枚数。
    #[must_use]
    pub fn total(&self) -> usize {
        self.cards.iter().map(|e| usize::from(e.count)).sum()
    }
}

/// [`DeckList::load`] のエラー。
#[derive(Debug)]
pub enum DeckLoadError {
    Io(std::io::Error),
    Yaml(serde_yaml::Error),
}

impl std::fmt::Display for DeckLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "deck io: {e}"),
            Self::Yaml(e) => write!(f, "deck parse: {e}"),
        }
    }
}

impl std::error::Error for DeckLoadError {}

impl From<std::io::Error> for DeckLoadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_yaml::Error> for DeckLoadError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Yaml(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_deck() {
        let yaml = "name: T\ncards:\n  - { slug: dreepy, count: 4 }\n  - { slug: fire-energy, count: 4 }\n";
        let d = DeckList::from_yaml_str(yaml).unwrap();
        assert_eq!(d.name.as_deref(), Some("T"));
        assert_eq!(d.total(), 8);
        assert_eq!(d.cards[0].slug, "dreepy");
    }

    #[test]
    fn name_is_optional() {
        let yaml = "cards:\n  - { slug: budew, count: 1 }\n";
        let d = DeckList::from_yaml_str(yaml).unwrap();
        assert!(d.name.is_none());
        // name None は serialize 時に skip。
        let json = serde_json::to_value(&d).unwrap();
        assert!(json.get("name").is_none());
    }
}
