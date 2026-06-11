//! wire protocol DTO (engine 非依存)。
//!
//! 本体 (PTCG AI Battle Platform) の `engine-runtime` protocol / state / action / event
//! DTO の serde 形を、engine クレートに依存せず写経したもの。サーバとやり取りする JSON の
//! 唯一の契約。`tests/wire_contract.rs` で本体と同一 JSON になることを固定する。

pub mod action;
pub mod event;
pub mod protocol;
pub mod state;
