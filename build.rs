//! 构建脚本：把 Windows 图标资源编译进可执行文件。
//!
//! 资源管理器读取的是可执行文件内的图标资源，运行时窗口图标由
//! `utils::egui_helper::load_app_icon` 加载同一份素材，两者因此保持一致。

fn main() {
    println!("cargo:rerun-if-changed=assets/bes.rc");
    println!("cargo:rerun-if-changed=assets/bes.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let _ = embed_resource::compile("assets/bes.rc", embed_resource::NONE);
}
