//! Compile only the UI used by the target platform.
fn main() {
    let entry = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        "ui/agent-companion.slint"
    } else {
        "ui/codex-tui.slint"
    };
    slint_build::compile(entry).expect("the Slint markup failed to compile");
}
