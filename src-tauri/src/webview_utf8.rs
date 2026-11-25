//! 强制前端资源 Content-Type 带 charset=utf-8。
//! 中文 Windows WebView2 在缺少 charset 时会按系统 ANSI（GBK）解码，
//! UTF-8 的 HTML/JS/CSS 就会整页中文乱码。Author: Corin-Sands

use std::borrow::Cow;

use http::header::{HeaderValue, CONTENT_TYPE};

pub fn ensure_utf8_content_type(response: &mut http::Response<Cow<'static, [u8]>>) {
    let Some(current) = response.headers().get(CONTENT_TYPE) else {
        return;
    };
    let Ok(value) = current.to_str() else {
        return;
    };
    let lower = value.to_ascii_lowercase();
    if lower.contains("charset=") {
        return;
    }
    let needs = lower.starts_with("text/")
        || lower == "application/javascript"
        || lower == "application/json"
        || lower.contains("+json")
        || lower.contains("javascript");
    if !needs {
        return;
    }
    let next = format!("{value}; charset=utf-8");
    if let Ok(hv) = HeaderValue::from_str(&next) {
        response.headers_mut().insert(CONTENT_TYPE, hv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Response;

    fn ctype_with(ct: &str) -> http::Response<Cow<'static, [u8]>> {
        Response::builder()
            .header(CONTENT_TYPE, ct)
            .body(Cow::Borrowed(&b""[..]))
            .unwrap()
    }

    #[test]
    fn appends_charset_to_html_js_css() {
        for ct in ["text/html", "text/javascript", "text/css", "application/json"] {
            let mut resp = ctype_with(ct);
            ensure_utf8_content_type(&mut resp);
            let got = resp.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
            assert_eq!(got, format!("{ct}; charset=utf-8"));
        }
    }

    #[test]
    fn leaves_existing_charset_and_binary() {
        let mut utf8 = ctype_with("text/html; charset=utf-8");
        ensure_utf8_content_type(&mut utf8);
        assert_eq!(
            utf8.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap(),
            "text/html; charset=utf-8"
        );
        let mut png = ctype_with("image/png");
        ensure_utf8_content_type(&mut png);
        assert_eq!(
            png.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap(),
            "image/png"
        );
    }
}
