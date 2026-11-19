//! `orbit-ocr-server`：远程 OCR 服务端。
//!
//! 用法：
//! ```text
//! orbit-ocr-server [--bind 127.0.0.1:8787] [--token <64hex>] [--data-dir <dir>]
//! ```
//! token 也可用环境变量 `ORBIT_OCR_TOKEN`；都未提供时随机生成并打印。

use orbit_ocr_server::{ServerConfig, generate_token};
use std::path::PathBuf;

fn main() {
    let mut bind = orbit_ocr_server::DEFAULT_BIND.to_string();
    let mut token = std::env::var("ORBIT_OCR_TOKEN").ok();
    let mut data_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("orbit-ocr-data");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => bind = next_value(&mut args, "--bind"),
            "--token" => token = Some(next_value(&mut args, "--token")),
            "--data-dir" => data_dir = PathBuf::from(next_value(&mut args, "--data-dir")),
            "--help" | "-h" => {
                println!("用法：orbit-ocr-server [--bind ADDR] [--token TOKEN] [--data-dir DIR]");
                return;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }

    let token = token.unwrap_or_else(|| {
        let token = generate_token();
        println!("未提供 token，已生成：{token}");
        token
    });
    let mut config = ServerConfig::new(token, data_dir);
    config.bind = bind;

    if let Err(error) = orbit_ocr_server::run(config) {
        eprintln!("服务端启动失败：{error}");
        std::process::exit(1);
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> String {
    args.next().unwrap_or_else(|| {
        eprintln!("{name} 缺少取值");
        std::process::exit(2);
    })
}
