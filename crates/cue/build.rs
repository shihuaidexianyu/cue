//! 把品牌图标(assets/cue.ico)嵌进 cue.exe 的 PE 资源:
//! 文件资源管理器图标、安装包快捷方式图标、托盘/窗口图标(cue-windows
//! 按资源 id 1 加载)全部同源。仅 Windows 有意义;其他平台无操作。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("cue.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
