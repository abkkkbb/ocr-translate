use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;
const REGION: &str = "cn-beijing";

/// A Volcengine OpenAPI endpoint: host, signing service name and action query.
struct Endpoint {
    host: &'static str,
    service: &'static str,
    query: &'static str,
}

const TRANSLATE: Endpoint = Endpoint {
    host: "translate.volcengineapi.com",
    service: "translate",
    query: "Action=TranslateText&Version=2025-03-01",
};
const BILL_DETAIL: Endpoint = Endpoint {
    host: "billing.volcengineapi.com",
    service: "billing",
    query: "Action=ListBillDetail&Version=2022-01-01",
};
const BALANCE: Endpoint = Endpoint {
    host: "billing.volcengineapi.com",
    service: "billing",
    query: "Action=QueryBalanceAcct&Version=2022-01-01",
};

/// Text translation is free for the first two million characters each month.
pub const FREE_CHARS_PER_MONTH: f64 = 2_000_000.0;

/// Texts per request; the API caps the list length.
const BATCH_SIZE: usize = 16;

pub fn translate(
    text: &str,
    target: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<String, String> {
    let translations = translate_list(&[text.to_owned()], target, access_key, secret_key)?;
    let text = translations.join("\n");
    if text.trim().is_empty() {
        Err("火山翻译返回了空译文。".into())
    } else {
        Ok(text)
    }
}

/// Translates each text separately, returning one translation per input in the same order.
pub fn translate_list(
    texts: &[String],
    target: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<Vec<String>, String> {
    if access_key.trim().is_empty() || secret_key.trim().is_empty() {
        return Err("请先填写火山引擎 Access Key 和 Secret Key。".into());
    }
    let mut translations = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(BATCH_SIZE) {
        let batch = request(chunk, target, access_key, secret_key)?;
        if batch.len() != chunk.len() {
            return Err("火山翻译返回的译文数量与原文不一致。".into());
        }
        translations.extend(batch);
    }
    Ok(translations)
}

fn request(
    texts: &[String],
    target: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<Vec<String>, String> {
    let body = serde_json::json!({ "TargetLanguage": target, "TextList": texts });
    parse_response(&post(
        &TRANSLATE,
        &body,
        access_key,
        secret_key,
        "火山翻译",
    )?)
}

/// Sends a signed POST and returns the response body. `label` names the call in errors.
fn post(
    endpoint: &Endpoint,
    body: &Value,
    access_key: &str,
    secret_key: &str,
    label: &str,
) -> Result<String, String> {
    let body = serde_json::to_string(body).map_err(|error| format!("创建火山请求失败：{error}"))?;
    let date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let (body_hash, authorization) = sign(endpoint, access_key, secret_key, &date, &body);
    let client = crate::net::client_builder()?
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("创建网络客户端失败：{error}"))?;
    let response = client
        .post(format!("https://{}/?{}", endpoint.host, endpoint.query))
        .header("Host", endpoint.host)
        .header("Content-Type", "application/json")
        .header("X-Date", date)
        .header("X-Content-Sha256", body_hash)
        .header("Authorization", authorization)
        .body(body)
        .send()
        .map_err(|error| format!("{label}请求失败：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("读取{label}响应失败：{error}"))?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value["ResponseMetadata"]["Error"]["Message"]
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| super::compact_error(&body));
        return Err(format!("{label}返回 {status}：{message}"));
    }
    Ok(body)
}

/// This month's text-translation usage and the account balance.
#[derive(Clone, Debug)]
pub struct Usage {
    pub used_chars: f64,
    pub balance: String,
    pub currency: String,
}

/// Bill details record free-tier usage as zero-cost rows; they lag real usage by hours.
pub fn query_usage(access_key: &str, secret_key: &str) -> Result<Usage, String> {
    const LABEL: &str = "火山费用查询";
    // Bill periods follow Beijing time.
    let period = (Utc::now() + chrono::Duration::hours(8))
        .format("%Y-%m")
        .to_string();
    let mut used_chars = 0.0;
    let mut offset = 0;
    loop {
        let body = serde_json::json!({
            "BillPeriod": period,
            "Limit": 100,
            "Offset": offset,
            "NeedRecordNum": 1,
        });
        let response = post(&BILL_DETAIL, &body, access_key, secret_key, LABEL)?;
        let (rows, chars, total) = parse_bill_detail(&response)?;
        used_chars += chars;
        offset += rows;
        if rows == 0 || offset >= total {
            break;
        }
    }
    let response = post(
        &BALANCE,
        &serde_json::json!({}),
        access_key,
        secret_key,
        LABEL,
    )?;
    let balance: Value =
        serde_json::from_str(&response).map_err(|error| format!("余额响应无效：{error}"))?;
    let result = &balance["Result"];
    Ok(Usage {
        used_chars,
        balance: result["AvailableBalance"]
            .as_str()
            .unwrap_or("0")
            .to_owned(),
        currency: result["Currency"].as_str().unwrap_or("CNY").to_owned(),
    })
}

/// Returns the row count, the text-translation characters in those rows, and the total row count.
fn parse_bill_detail(body: &str) -> Result<(usize, f64, usize), String> {
    let response: Value =
        serde_json::from_str(body).map_err(|error| format!("账单响应无效：{error}"))?;
    if let Some(error) = response["ResponseMetadata"]["Error"]["Message"].as_str() {
        return Err(format!("火山费用查询失败：{error}"));
    }
    let result = &response["Result"];
    let rows = result["List"].as_array().cloned().unwrap_or_default();
    let number = |value: &Value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    };
    let chars = rows
        .iter()
        .filter(|row| row["ProductZh"].as_str() == Some("机器翻译"))
        .filter(|row| row["Unit"].as_str() == Some("百万字符"))
        .filter_map(|row| number(&row["Count"]))
        .sum::<f64>()
        * 1_000_000.0;
    let total = number(&result["Total"]).map_or(rows.len(), |total| total as usize);
    Ok((rows.len(), chars, total))
}

fn sign(endpoint: &Endpoint, ak: &str, sk: &str, timestamp: &str, body: &str) -> (String, String) {
    let Endpoint {
        host,
        service,
        query,
    } = endpoint;
    let date = &timestamp[..8];
    let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
    let headers = format!(
        "content-type:application/json\nhost:{host}\nx-content-sha256:{body_hash}\nx-date:{timestamp}\n"
    );
    let signed_headers = "content-type;host;x-content-sha256;x-date";
    let canonical = format!("POST\n/\n{query}\n{headers}\n{signed_headers}\n{body_hash}");
    let scope = format!("{date}/{REGION}/{service}/request");
    let to_sign = format!(
        "HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let key_date = mac(sk.as_bytes(), date);
    let key_region = mac(&key_date, REGION);
    let key_service = mac(&key_region, service);
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

fn parse_response(body: &str) -> Result<Vec<String>, String> {
    let response: Value =
        serde_json::from_str(body).map_err(|error| format!("火山翻译响应无效：{error}"))?;
    if let Some(error) = response["ResponseMetadata"]["Error"]["Message"].as_str() {
        return Err(format!("火山翻译失败：{error}"));
    }
    let translations = response["Result"]["TranslationList"]
        .as_array()
        .or_else(|| response["TranslationList"].as_array())
        .ok_or_else(|| format!("火山翻译没有返回译文：{}", super::compact_error(body)))?;
    Ok(translations
        .iter()
        .map(|item| item["Translation"].as_str().unwrap_or_default().to_owned())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_wrapper() {
        assert_eq!(
            parse_response(
                r#"{"Result":{"TranslationList":[{"Translation":"你好"},{"Translation":"世界"}]}}"#
            )
            .unwrap(),
            ["你好", "世界"]
        );
    }

    #[test]
    fn sums_text_translation_usage_from_bill_rows() {
        let body = r#"{"Result":{"Total":3,"List":[
            {"ProductZh":"机器翻译","Unit":"百万字符","Count":"0.016207"},
            {"ProductZh":"机器翻译","Unit":"张","Count":"5"},
            {"ProductZh":"云服务器","Unit":"小时","Count":"2"}]}}"#;
        let (rows, chars, total) = parse_bill_detail(body).unwrap();
        assert_eq!((rows, total), (3, 3));
        assert!((chars - 16_207.0).abs() < 0.5);
    }

    #[test]
    fn signs_request_without_leaking_secret() {
        let (_, authorization) = sign(
            &TRANSLATE,
            "example-ak",
            "example-sk",
            "20261009T050000Z",
            "{}",
        );
        assert!(authorization.starts_with(
            "HMAC-SHA256 Credential=example-ak/20261009/cn-beijing/translate/request"
        ));
        assert!(!authorization.contains("example-sk"));
        assert!(authorization.ends_with(
            "Signature=1fe3c7bb49b9e0063bbf832b6bef03b0745a26cbdc2bb77dfb3888d78abfd412"
        ));
    }
}
