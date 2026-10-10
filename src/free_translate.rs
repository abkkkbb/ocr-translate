use crate::TranslationMode;
use serde_json::Value;
use std::time::Duration;

pub fn translate(text: &str, target: &str, mode: TranslationMode) -> Result<String, String> {
    let language = target_code(target, mode)?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36")
        .build()
        .map_err(|error| format!("创建网络客户端失败：{error}"))?;

    match mode {
        TranslationMode::Tencent => translate_tencent(&client, text, language),
        TranslationMode::Google => translate_google(&client, text, language),
        TranslationMode::Volcengine => unreachable!("火山翻译使用签名请求"),
        TranslationMode::Vision => unreachable!("视觉模型不使用免费文本翻译接口"),
    }
}

pub(crate) fn target_code(target: &str, mode: TranslationMode) -> Result<&'static str, String> {
    let chinese = if mode == TranslationMode::Google {
        "zh-CN"
    } else {
        "zh"
    };
    match target.trim().to_lowercase().as_str() {
        "简体中文" | "中文" | "zh" | "zh-cn" => Ok(chinese),
        "繁体中文" | "zh-tw" => Ok("zh-TW"),
        "英语" | "英文" | "en" => Ok("en"),
        "日语" | "日文" | "ja" => Ok("ja"),
        "韩语" | "韩文" | "ko" => Ok("ko"),
        "法语" | "fr" => Ok("fr"),
        "西班牙语" | "es" => Ok("es"),
        "俄语" | "ru" => Ok("ru"),
        "德语" | "de" => Ok("de"),
        other => Err(format!("当前翻译渠道暂不支持目标语言：{other}")),
    }
}

fn translate_tencent(
    client: &reqwest::blocking::Client,
    text: &str,
    target: &str,
) -> Result<String, String> {
    let body = serde_json::json!({
        "header": {
            "fn": "auto_translation_block",
            "client_key": "browser-chrome-110.0.0-Mac OS-df4bd4c5-a65d-44b2-a40f-42f34f3535f2-1677486696487"
        },
        "type": "plain",
        "model_category": "normal",
        "source": { "lang": "auto", "text_block": text },
        "target": { "lang": target }
    });
    let response = client
        .post("https://transmart.qq.com/api/imt")
        .header("Referer", "https://yi.qq.com/zh-CN/index")
        .json(&body)
        .send()
        .map_err(|error| format!("腾讯翻译请求失败：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("读取腾讯翻译响应失败：{error}"))?;
    if !status.is_success() {
        return Err(format!(
            "腾讯翻译返回 {status}：{}",
            super::compact_error(&body)
        ));
    }
    parse_tencent(&body)
}

fn parse_tencent(body: &str) -> Result<String, String> {
    let response: Value =
        serde_json::from_str(body).map_err(|error| format!("腾讯翻译响应无效：{error}"))?;
    response["auto_translation"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("腾讯翻译没有返回译文：{}", super::compact_error(body)))
}

fn translate_google(
    client: &reqwest::blocking::Client,
    text: &str,
    target: &str,
) -> Result<String, String> {
    let response = client
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&[
            ("client", "gtx"),
            ("dt", "t"),
            ("dj", "1"),
            ("ie", "UTF-8"),
            ("oe", "UTF-8"),
            ("sl", "auto"),
            ("tl", target),
            ("q", text),
        ])
        .send()
        .map_err(|error| format!("Google 翻译请求失败：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("读取 Google 翻译响应失败：{error}"))?;
    if !status.is_success() {
        return Err(format!(
            "Google 翻译返回 {status}：{}",
            super::compact_error(&body)
        ));
    }
    parse_google(&body)
}

fn parse_google(body: &str) -> Result<String, String> {
    let response: Value =
        serde_json::from_str(body).map_err(|error| format!("Google 翻译响应无效：{error}"))?;
    let sentences = response["sentences"]
        .as_array()
        .ok_or("Google 翻译没有返回句子列表。")?;
    let translated: String = sentences
        .iter()
        .filter_map(|sentence| sentence["trans"].as_str())
        .collect();
    if translated.trim().is_empty() {
        Err("Google 翻译没有返回译文。".into())
    } else {
        Ok(translated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tencent_response() {
        assert_eq!(
            parse_tencent(r#"{"auto_translation":"你好，世界"}"#).unwrap(),
            "你好，世界"
        );
    }

    #[test]
    fn parses_google_sentences() {
        assert_eq!(
            parse_google(r#"{"sentences":[{"trans":"你好"},{"trans":"世界"}]}"#).unwrap(),
            "你好世界"
        );
    }

    #[test]
    fn maps_chinese_target_for_each_provider() {
        assert_eq!(
            target_code("简体中文", TranslationMode::Tencent).unwrap(),
            "zh"
        );
        assert_eq!(
            target_code("简体中文", TranslationMode::Google).unwrap(),
            "zh-CN"
        );
    }

    #[test]
    #[ignore = "uses Tencent's live website translation endpoint"]
    fn translates_live_tencent_text() {
        let result = translate("Hello world", "简体中文", TranslationMode::Tencent).unwrap();
        assert!(result.contains('世') || result.contains('你'));
    }
}
