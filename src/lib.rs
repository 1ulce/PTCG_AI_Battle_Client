//! PTCG AI Battle Platform 向けの Dragapult ex 参照 bot + 薄い WebSocket クライアント。
//!
//! 本リポジトリは engine (ルールエンジン) には依存しない。サーバとは protocol JSON
//! ([`wire`]) だけでやり取りし、カード事実は pokemon-card-data から [`cards::CardFacts`]
//! で読む。bot ロジック ([`bots`]) は本体 (PTCG AI Battle Platform) の内蔵 bot を engine
//! 非依存型へ載せ替えたもの。
//!
//! ## 使い方
//!
//! `connect` バイナリがリモート `serve` に接続して 1 局ずつ対戦する。詳細は README 参照。

pub mod bots;
pub mod cards;
pub mod deck;
pub mod transport;
pub mod wire;
