//! The HTTP client shared by every translation channel, applying the proxy setting.

use serde::{Deserialize, Serialize};
use std::{
    sync::{Mutex, PoisonError, RwLock},
    thread,
    time::Duration,
};
use windows::{
    core::{w, PCWSTR},
    Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_ANY},
};

/// A desktop browser's user agent, for the keyless web endpoints that expect one.
pub const BROWSER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36";

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

/// What a client was built for: the proxy setting and, in system mode, the Windows proxy
/// values reqwest reads, so turning a proxy app's system proxy on or off still takes effect.
#[derive(PartialEq)]
struct ClientKey {
    setting: ProxySetting,
    system: Vec<Vec<u8>>,
}

static CLIENT: Mutex<Option<(ClientKey, reqwest::blocking::Client)>> = Mutex::new(None);

/// The client every request shares. Reusing it keeps connections alive between requests,
/// which saves a TLS handshake (often through a proxy) on each one. Callers set their own
/// request timeouts.
pub fn client() -> Result<reqwest::blocking::Client, String> {
    let setting = PROXY.read().map(|proxy| proxy.clone()).unwrap_or_default();
    let system = if setting.mode == ProxyMode::System {
        system_proxy()
    } else {
        Vec::new()
    };
    let key = ClientKey { setting, system };
    let mut shared = CLIENT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((built_for, client)) = shared.as_ref() {
        if *built_for == key {
            return Ok(client.clone());
        }
    }
    let builder = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(15));
    let builder = match key.setting.mode {
        ProxyMode::System => builder,
        ProxyMode::None => builder.no_proxy(),
        ProxyMode::Custom => {
            validate_proxy_url(&key.setting.url)?;
            let proxy = reqwest::Proxy::all(key.setting.url.trim())
                .map_err(|error| format!("代理地址无效：{error}"))?;
            builder.proxy(proxy)
        }
    };
    let client = builder
        .build()
        .map_err(|error| format!("创建网络客户端失败：{error}"))?;
    let previous = shared.replace((key, client.clone()));
    drop(shared);
    // The last handle of an old client waits for its worker thread; not while holding the lock.
    drop(previous);
    Ok(client)
}

/// Connects to the server of `url` ahead of a request about to be made, such as while the
/// user is still selecting a screen region. The response itself does not matter.
pub fn warm_up(url: String) {
    thread::spawn(move || {
        if let Ok(client) = client() {
            let _ = client.head(url).timeout(Duration::from_secs(10)).send();
        }
    });
}

/// The per-user Windows proxy values that reqwest's system proxy support reads.
fn system_proxy() -> Vec<Vec<u8>> {
    const KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    [w!("ProxyEnable"), w!("ProxyServer"), w!("ProxyOverride")]
        .into_iter()
        .map(|name| {
            let mut data = vec![0u8; 2048];
            let mut size = data.len() as u32;
            let read = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    KEY,
                    name,
                    RRF_RT_ANY,
                    None,
                    Some(data.as_mut_ptr().cast()),
                    Some(&mut size),
                )
            };
            data.truncate(if read.is_ok() { size as usize } else { 0 });
            data
        })
        .collect()
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

    #[test]
    fn reads_the_system_proxy_values_the_shared_client_is_keyed_by() {
        let values = system_proxy();
        assert_eq!(values.len(), 3);
        assert_eq!(values, system_proxy());
    }
}
