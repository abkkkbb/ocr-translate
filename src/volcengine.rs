use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;
const HOST: &str = "translate.volcengineapi.com";
const REGION: &str = "cn-beijing";
const SERVICE: &str = "translate";
const QUERY: &str = "Action=TranslateText&Version=2025-03-01";

pub fn translate(
    text: &str,
    target: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<String, String> {
    if access_key.trim().is_empty() || secret_key.trim().is_empty() {
        return Err("请先填写火山引擎 Access Key 和 Secret Key。".into());
    }
    let body = serde_json::json!({ "TargetLanguage": target, "TextList": [text] });
    let body =
        serde_json::to_string(&body).map_err(|error| format!("创建火山请求失败：{error}"))?;
    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let (body_hash, authorization) = sign(access_key, secret_key, &date, &body);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("创建网络客户端失败：{error}"))?;
    let response = client
        .post(format!("https://{HOST}/?{QUERY}"))
        .header("Host", HOST)
        .header("Content-Type", "application/json")
        .header("X-Date", date)
        .header("X-Content-Sha256", body_hash)
        .header("Authorization", authorization)
        .body(body)
        .send()
        .map_err(|error| format!("火山翻译请求失败：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("读取火山翻译响应失败：{error}"))?;
    if !status.is_success() {
        return Err(format!(
            "火山翻译返回 {status}：{}",
            super::compact_error(&body)
        ));
    }
    parse_response(&body)
}

fn sign(ak: &str, sk: &str, timestamp: &str, body: &str) -> (String, String) {
    let date = &timestamp[..8];
    let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
    let headers = format!(
        "content-type:application/json\nhost:{HOST}\nx-content-sha256:{body_hash}\nx-date:{timestamp}\n"
    );
    let signed_headers = "content-type;host;x-content-sha256;x-date";
    let canonical = format!("POST\n/\n{QUERY}\n{headers}\n{signed_headers}\n{body_hash}");
    let scope = format!("{date}/{REGION}/{SERVICE}/request");
    let to_sign = format!(
        "HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let key_date = mac(sk.as_bytes(), date);
    let key_region = mac(&key_date, REGION);
    let key_service = mac(&key_region, SERVICE);
    let signing_key = mac(&key_service, "request");
    let signature = hex::encode(mac(&signing_key, &to_sign));
    let authorization = format!(
        "HMAC-SHA256 Credential={ak}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );
    (body_hash, authorization)
}

fn mac(key: &[u8], message: &str) -> Vec<u8> {
    let mut signer = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    signer.update(message.as_bytes());
    signer.finalize().into_bytes().to_vec()
}

fn parse_response(body: &str) -> Result<String, String> {
    let response: Value =
        serde_json::from_str(body).map_err(|error| format!("火山翻译响应无效：{error}"))?;
    if let Some(error) = response["ResponseMetadata"]["Error"]["Message"].as_str() {
        return Err(format!("火山翻译失败：{error}"));
    }
    let translations = response["Result"]["TranslationList"]
        .as_array()
        .or_else(|| response["TranslationList"].as_array())
        .ok_or_else(|| format!("火山翻译没有返回译文：{}", super::compact_error(body)))?;
    let text = translations
        .iter()
        .filter_map(|item| item["Translation"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        Err("火山翻译返回了空译文。".into())
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_wrapper() {
        assert_eq!(
            parse_response(r#"{"Result":{"TranslationList":[{"Translation":"你好"}]}}"#).unwrap(),
            "你好"
        );
    }

    #[test]
    fn signs_request_without_leaking_secret() {
        let (_, authorization) = sign("example-ak", "example-sk", "20261009T050000Z", "{}");
        assert!(authorization.starts_with(
            "HMAC-SHA256 Credential=example-ak/20261009/cn-beijing/translate/request"
        ));
        assert!(!authorization.contains("example-sk"));
        assert!(authorization.ends_with(
            "Signature=1fe3c7bb49b9e0063bbf832b6bef03b0745a26cbdc2bb77dfb3888d78abfd412"
        ));
    }
}
