//! 极简 HTTP/1.1 解析与响应（零依赖）。
//!
//! 只覆盖本服务需要的子集：请求行 + 头部 + `Content-Length` 正文，
//! 响应固定 `Connection: close`。TLS 由反向代理终止（见 docs/REMOTE_OCR.md）。

use std::io::{BufRead, BufReader, Read, Write};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn content_length(&self) -> Result<usize, String> {
        match self.header("content-length") {
            None => Ok(0),
            Some(raw) => raw
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("Content-Length 不是整数：{raw}")),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum HttpError {
    BadRequest(String),
    TooLarge,
    Io(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::BadRequest(message) => write!(formatter, "请求格式错误：{message}"),
            HttpError::TooLarge => write!(formatter, "请求体超出限制"),
            HttpError::Io(message) => write!(formatter, "读取请求失败：{message}"),
        }
    }
}

/// 读取一个完整请求。`max_body` 为允许的正文上限（字节）。
pub fn read_request(stream: &mut impl Read, max_body: usize) -> Result<Request, HttpError> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|error| HttpError::Io(error.to_string()))?;
    if request_line.trim().is_empty() {
        return Err(HttpError::BadRequest("空请求行".to_string()));
    }
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| HttpError::BadRequest("缺少方法".to_string()))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| HttpError::BadRequest("缺少路径".to_string()))?
        .to_string();

    let mut headers = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| HttpError::Io(error.to_string()))?;
        if read == 0 {
            return Err(HttpError::Io("连接在头部结束前关闭".to_string()));
        }
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            return Err(HttpError::TooLarge);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed
            .split_once(':')
            .ok_or_else(|| HttpError::BadRequest(format!("头部缺少冒号：{trimmed}")))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }

    let mut request = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let length = request
        .content_length()
        .map_err(HttpError::BadRequest)?;
    if length > max_body {
        return Err(HttpError::TooLarge);
    }
    if length > 0 {
        let mut body = vec![0u8; length];
        reader
            .read_exact(&mut body)
            .map_err(|error| HttpError::Io(error.to_string()))?;
        request.body = body;
    }
    Ok(request)
}

pub fn write_response(
    stream: &mut impl Write,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8]) -> Result<Request, HttpError> {
        read_request(&mut std::io::Cursor::new(raw.to_vec()), 1024)
    }

    #[test]
    fn parses_method_path_headers_and_body() {
        let raw = b"POST /v1/ocr HTTP/1.1\r\nAuthorization: Bearer abc\r\nContent-Length: 3\r\n\r\nxyz";
        let request = parse(raw).expect("应解析成功");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/ocr");
        assert_eq!(request.header("authorization"), Some("Bearer abc"));
        assert_eq!(request.body, b"xyz");
    }

    #[test]
    fn rejects_missing_content_length_bytes() {
        let raw = b"POST /v1/ocr HTTP/1.1\r\nContent-Length: 5\r\n\r\nab";
        assert!(matches!(parse(raw), Err(HttpError::Io(_))));
    }

    #[test]
    fn rejects_oversized_body() {
        let raw = b"POST /v1/ocr HTTP/1.1\r\nContent-Length: 2048\r\n\r\n";
        assert_eq!(parse(raw).unwrap_err(), HttpError::TooLarge);
    }

    #[test]
    fn rejects_header_without_colon() {
        let raw = b"POST / HTTP/1.1\r\nBroken\r\n\r\n";
        assert!(matches!(parse(raw), Err(HttpError::BadRequest(_))));
    }

    #[test]
    fn writes_a_complete_response() {
        let mut output = Vec::new();
        write_response(&mut output, 200, "application/json", b"{}").unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }
}
