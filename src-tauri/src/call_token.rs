// Author: Corin-Sands
//! 用用户订阅 token 换取临时调用钥（phllm_ / ngmk_）。

use std::time::Duration;

use serde::Deserialize;

/// 允许的订阅主机（https://{host}/clash/<token>）。
/// call-token 一律打到 [`CANONICAL_HUB_BASE`]。
const ALLOWED_HOSTS: &[&str] = &["supone.top", "www.supone.top"];

/// 换取 call-token 的规范 Hub（本仓库唯一对外 Hub）。Author: Corin-Sands
const CANONICAL_HUB_BASE: &str = "https://supone.top";

const SUB_TOKEN_HINT: &str =
    "请先拿到一个授权的订阅 token（形如 https://supone.top/clash/<token>），并在「订阅」页导入";

#[derive(Debug, Clone)]
pub struct ParsedSubscription {
    pub token: String,
    /// 始终为 `https://supone.top`（不跟订阅 URL 走弯路）。
    pub hub_base: String,
}

#[derive(Debug, Deserialize)]
pub struct CallTokenResponse {
    pub call_token: String,
    pub expires_at: String,
    pub base_url: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub proj_id: i64,
    /// Hub users.id；对话请求编进 NeuroGate `user`。
    #[serde(default)]
    pub user_id: i64,
}

/// NeuroGate Chat Completions 的 `user`：Hub 用户 + 本机会话键。
pub fn neurogate_user_key(hub_user_id: i64, conversation_user: &str) -> String {
    format!("supone-u{hub_user_id}:{conversation_user}")
}

/// 从订阅 URL 解析 token；Hub 基址固定为 supone.top。
pub fn parse_subscription_token(url: &str) -> Result<ParsedSubscription, String> {
    let url = url.trim().trim_end_matches('/');
    for host in ALLOWED_HOSTS {
        for scheme in ["https", "http"] {
            let prefix = format!("{scheme}://{host}/clash/");
            if let Some(rest) = url.strip_prefix(&prefix) {
                if rest.is_empty()
                    || rest.contains('/')
                    || rest.contains('?')
                    || rest.contains('#')
                    || rest.chars().any(char::is_whitespace)
                {
                    return Err(SUB_TOKEN_HINT.into());
                }
                return Ok(ParsedSubscription {
                    token: rest.to_string(),
                    hub_base: CANONICAL_HUB_BASE.to_string(),
                });
            }
        }
    }
    Err(SUB_TOKEN_HINT.into())
}

pub fn missing_subscription_hint() -> String {
    SUB_TOKEN_HINT.into()
}

/// 距失效不足 5 分钟视为需重签。
pub fn call_token_fresh(expires_at: &str) -> bool {
    let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expires_at.trim()) else {
        return false;
    };
    let skew = chrono::Duration::minutes(5);
    exp.with_timezone(&chrono::Utc) > chrono::Utc::now() + skew
}

pub async fn exchange_call_token(
    subscription_token: &str,
    hub_base: &str,
) -> Result<CallTokenResponse, String> {
    let token = subscription_token.trim();
    if token.is_empty() {
        return Err(SUB_TOKEN_HINT.into());
    }
    let hub_base = hub_base.trim().trim_end_matches('/');
    if hub_base.is_empty() {
        return Err(SUB_TOKEN_HINT.into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .post(format!("{hub_base}/api/neurogate/call-token"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .map_err(|err| format!("换取调用 token 失败: {err}"))?;
    let status = response.status();
    let raw = response
        .text()
        .await
        .map_err(|err| format!("换取调用 token 失败: {err}"))?;
    if status.as_u16() == 404 {
        return Err("订阅 token 无效、已停用或已过期，请先拿到一个授权的订阅 token".into());
    }
    if status.as_u16() == 422 {
        return Err("订阅 token 格式非法".into());
    }
    if status.as_u16() == 429 {
        return Err("换取调用 token 过于频繁，请稍后再试".into());
    }
    if status.as_u16() == 502 {
        return Err("Hub 签发 NeuroGate 调用 token 失败，请稍后再试".into());
    }
    if !status.is_success() {
        return Err(format!(
            "换取调用 token 失败: HTTP {status} {}",
            short(&raw)
        ));
    }
    let parsed: CallTokenResponse = serde_json::from_str(&raw)
        .map_err(|err| format!("换取调用 token 响应无效: {err}"))?;
    if !(parsed.call_token.starts_with("phllm_") || parsed.call_token.starts_with("ngmk_")) {
        return Err("换取调用 token 响应无效: 非 phllm_/ngmk_ 钥".into());
    }
    if parsed.base_url.trim().is_empty() {
        return Err("换取调用 token 响应无效: 缺少 base_url".into());
    }
    if parsed.user_id <= 0 {
        return Err("换取调用 token 响应无效: 缺少 user_id（请升级 Hub）".into());
    }
    Ok(parsed)
}

fn short(text: &str) -> String {
    const LIMIT: usize = 180;
    let compact: String = text.chars().take(LIMIT).collect();
    if text.chars().count() > LIMIT {
        format!("{compact}…")
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_family_and_canonical_hub() {
        for url in [
            "https://supone.top/clash/example_token",
            "https://www.supone.top/clash/example_token",
        ] {
            let parsed = parse_subscription_token(url).unwrap();
            assert_eq!(parsed.token, "example_token");
            assert_eq!(parsed.hub_base, "https://supone.top");
        }
        let with_slash =
            parse_subscription_token("https://supone.top/clash/example_token/").unwrap();
        assert_eq!(with_slash.token, "example_token");
        assert_eq!(with_slash.hub_base, "https://supone.top");
        assert!(parse_subscription_token("https://supone.top/clash/example_token").is_err());
    }

    #[test]
    fn rejects_non_allowed_or_bad_path() {
        assert!(parse_subscription_token("https://example.com/clash/abc").is_err());
        assert!(parse_subscription_token("https://supone.top/clash/").is_err());
        assert!(parse_subscription_token("https://supone.top/clash/a/b").is_err());
        assert!(parse_subscription_token("example_token").is_err());
    }

    #[test]
    fn neurogate_user_key_embeds_hub_user() {
        assert_eq!(neurogate_user_key(42, "conv-abc"), "supone-u42:conv-abc");
    }
}
