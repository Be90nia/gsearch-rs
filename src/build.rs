//! q01 版本烧录：build.rs（vergen-gitcl）编译期注入 GIT_DESCRIBE/GIT_SHA，这里组装 `--version` 行。

use std::sync::LazyLock;

/// `0.2.9 (git:<describe>)`；非 git 构建（release tarball，env 缺席）→ `0.2.9 (dev)`。
/// 不带 "gsearch " 前缀——clap 的 `--version` 输出自带 `{name} ` 前缀（main.rs name 属性）。
/// describe 带 `--always`：无标签仓库也会产出短 sha，故 SHA 兜底仅在完全无 git 时触达。
pub fn version_line() -> &'static str {
    static LINE: LazyLock<String> = LazyLock::new(|| {
        let v = env!("CARGO_PKG_VERSION");
        match option_env!("VERGEN_GIT_DESCRIBE").or(option_env!("VERGEN_GIT_SHA")) {
            Some(id) => format!("{v} (git:{id})"),
            None => format!("{v} (dev)"),
        }
    });
    &LINE
}
