//! HTTP client construction shared by every translation channel, applying the proxy setting.

use serde::{Deserialize, Serialize};
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// Use the proxy configured in Windows settings (as Clash's "system proxy" sets it).
    #[default]
    System,
    None,
    Custom,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxySetting {
    pub mode: ProxyMode,
    pub url: String,
}

static PROXY: RwLock<ProxySetting> = RwLock::new(ProxySetting {
    mode: ProxyMode::System,
    url: String::new(),
});

/// Applies to clients built afterwards; requests already running keep their proxy.
pub fn set_proxy(setting: ProxySetting) {
    if let Ok(mut proxy) = PROXY.write() {
        *proxy = setting;
    }
}

/// Checks a custom proxy address such as `http://127.0.0.1:7890` or `socks5://127.0.0.1:7890`.
pub fn validate_proxy_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    let scheme = url
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    if !matches!(
        scheme.as_deref(),
        Some("http" | "https" | "socks5" | "socks5h")
    ) {
        return Err(
            "代理地址需以 http://、https:// 或 socks5:// 开头，例如 http://127.0.0.1:7890。".into(),
        );
    }
    reqwest::Proxy::all(url)
        .map(|_| ())
        .map_err(|error| format!("代理地址无效：{error}"))
}

pub fn client_builder() -> Result<reqwest::blocking::ClientBuilder, String> {
    let setting = PROXY.read().map(|proxy| proxy.clone()).unwrap_or_default();
    let builder = reqwest::blocking::Client::builder();
    match setting.mode {
        ProxyMode::System => Ok(builder),
        ProxyMode::None => Ok(builder.no_proxy()),
        ProxyMode::Custom => {
            validate_proxy_url(&setting.url)?;
            let proxy = reqwest::Proxy::all(setting.url.trim())
                .map_err(|error| format!("代理地址无效：{error}"))?;
            Ok(builder.proxy(proxy))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_socks_proxies() {
        assert!(validate_proxy_url("http://127.0.0.1:7890").is_ok());
        assert!(validate_proxy_url(" socks5://127.0.0.1:7890 ").is_ok());
    }

    #[test]
    fn rejects_addresses_without_a_scheme() {
        assert!(validate_proxy_url("127.0.0.1:7890").is_err());
        assert!(validate_proxy_url("ftp://127.0.0.1:21").is_err());
        assert!(validate_proxy_url("").is_err());
    }
}
