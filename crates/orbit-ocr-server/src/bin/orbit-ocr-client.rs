//! `orbit-ocr-client`：截屏（或读本地图片）→ 上传 → 打印 OCR 结果。
//!
//! 用法：
//! ```text
//! orbit-ocr-client --server 127.0.0.1:8787 --token <hex> --capture
//! orbit-ocr-client --server 127.0.0.1:8787 --token <hex> --image shot.png
//! ```
//! 这是显式运行的一次性命令：不常驻、不隐藏、可随时不用。

use orbit_ocr_server::upload_png;
use std::io::Read;

fn main() {
    let mut server = orbit_ocr_server::DEFAULT_BIND.to_string();
    let mut token: Option<String> = std::env::var("ORBIT_OCR_TOKEN").ok();
    let mut mode: Option<(String, Vec<u8>)> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => server = value(&mut args, "--server"),
            "--token" => token = Some(value(&mut args, "--token")),
            "--capture" => {
                let png = orbit_ocr_server::capture_primary_screen_png()
                    .unwrap_or_else(|error| fail(&format!("截屏失败：{error}")));
                mode = Some(("capture".to_string(), png));
            }
            "--image" => {
                let path = value(&mut args, "--image");
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .and_then(|mut file| file.read_to_end(&mut bytes))
                    .unwrap_or_else(|error| fail(&format!("读取图片 {path} 失败：{error}")));
                mode = Some((path, bytes));
            }
            "--help" | "-h" => {
                println!(
                    "用法：orbit-ocr-client [--server ADDR] [--token TOKEN] (--capture | --image FILE)"
                );
                return;
            }
            other => fail(&format!("未知参数：{other}")),
        }
    }

    let Some((source, png)) = mode else {
        fail("必须提供 --capture 或 --image FILE");
    };
    let token = token.unwrap_or_else(|| fail("缺少 token（--token 或 ORBIT_OCR_TOKEN）"));

    let started = std::time::Instant::now();
    let response = upload_png(&server, &token, &png)
        .unwrap_or_else(|error| fail(&format!("上传失败：{error}")));
    let elapsed = started.elapsed().as_millis();

    if !response.ok {
        fail(&format!(
            "服务端返回错误：{}",
            response.error.unwrap_or_else(|| "未知错误".to_string())
        ));
    }
    let result = response.result.unwrap_or_else(|| fail("服务端没有返回 OCR 结果"));
    println!("来源：{source}（{} 字节，往返 {elapsed} ms）", png.len());
    if let Some(image) = response.image {
        println!("服务端保存：{}", image.display());
    }
    println!("文本：{}", result.text);
    println!("行数：{}，textAngle：{:?}", result.lines.len(), result.text_angle);
    for (index, line) in result.lines.iter().enumerate() {
        println!(
            "  [{index}] bbox={:?} text={:?} words={}",
            line.bbox,
            line.text,
            line.words.len()
        );
    }
}

fn value(args: &mut impl Iterator<Item = String>, name: &str) -> String {
    args.next().unwrap_or_else(|| fail(&format!("{name} 缺少取值")))
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}
