fn main() {
    // Windows exe 图标在编译期由 tauri-build 写入 .res 资源。
    // 显式声明依赖，确保更换 icon.ico 后 build script 重新执行（否则 cargo 会复用旧的 resource.lib）。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    // helper-manifest/orbit-elevated-helper.manifest 目前未被嵌入：提权由
    // src/elevated.rs 的 PowerShell `-Verb RunAs` 完成。若要启用内置 manifest，
    // 需在此为 helper 二进制单独写入资源，避免把主进程也提升为管理员。
    tauri_build::build()
}
