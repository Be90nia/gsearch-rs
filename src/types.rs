//! 全链路统一搜索结果结构（M1 仅占位定义，M2 填 SERP 解析逻辑）

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}
