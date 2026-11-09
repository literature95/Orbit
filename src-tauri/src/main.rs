// Windows 下不创建控制台窗口（dev / release 均不弹终端）
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    orbit_desktop_lib::run();
}
