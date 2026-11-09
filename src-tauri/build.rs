fn main() {
    // Windows exe 图标在编译期由 tauri-build 写入 .res 资源。
    // 显式声明依赖，确保更换 icon.ico 后 build script 重新执行（否则 cargo 会复用旧的 resource.lib）。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
