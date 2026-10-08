//! q01 版本烧录：编译期把 git describe/短 sha 注入 `VERGEN_GIT_DESCRIBE`/`VERGEN_GIT_SHA`，
//! src/build.rs 在运行时组装 `--version` 行。
//! 无 .git（release tarball）构建：Emitter 默认不 fail——指令缺席 + cargo:warning，
//! 代码侧 `option_env!` 兜底 `(dev)`。
use vergen_gitcl::{Emitter, Gitcl};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gitcl = Gitcl::builder()
        // (tags, dirty, match_pattern)：tags=true 让轻量标签也参与 describe；
        // 不追加 dirty 后缀（版本行要稳定）；--always 兜底使无标签仓库也产出短 sha。
        .describe(true, false, None)
        // VERGEN_GIT_SHA 用短 sha，与 describe 尾段形态一致
        .sha(true)
        .build();
    Emitter::default().add_instructions(&gitcl)?.emit()?;
    Ok(())
}
