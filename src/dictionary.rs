//! Dictionary details (phonetics, parts of speech, word forms, examples) for a single English
//! word, shown alongside its translation. Youdao comes first; Google's dictionary data is the
//! fallback.

use serde_json::Value;
use std::time::Duration;

/// Meanings per part of speech before the rest is cut off.
const MAX_MEANINGS: usize = 8;
const MAX_EXAMPLES: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub word: String,
    pub uk_phonetic: Option<String>,
    pub us_phonetic: Option<String>,
    /// Part of speech such as "v." (may be empty) and its meanings.
    pub senses: Vec<(String, String)>,
    /// Inflections such as ("过去式", "ran").
    pub forms: Vec<(String, String)>,
    /// English sentence and its Chinese translation.
    pub examples: Vec<(String, String)>,
}

/// One English word (letters with inner hyphens or apostrophes), the only input looked up.
pub fn is_single_english_word(text: &str) -> bool {
    let word = text.trim();
    let mut chars = word.chars();
    !word.is_empty()
        && word.len() <= 40
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && word.chars().last().is_some_and(|c| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '\'')
}

/// Returns `None` when neither source knows the word, e.g. a misspelling.
pub fn lookup(word: &str) -> Option<Entry> {
    let word = word.trim();
    let client = crate::net::client_builder()
        .ok()?
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36")
        .build()
        .ok()?;
    let youdao = client
        .get("https://dict.youdao.com/jsonapi")
        .query(&[("q", word), ("le", "en")])
        .send()
        .and_then(|response| response.text())
        .ok()
        .and_then(|body| parse_youdao(word, &body));
    youdao.or_else(|| {
        client
            .get("https://translate.googleapis.com/translate_a/single")
            .query(&[
                ("client", "gtx"),
                ("sl", "en"),
                ("tl", "zh-CN"),
                ("dt", "bd"),
                ("dj", "1"),
                ("q", word),
            ])
            .send()
            .and_then(|response| response.text())
            .ok()
            .and_then(|body| parse_google(word, &body))
    })
}

/// Splits "v. 跑，奔跑" into ("v.", "跑，奔跑"); lines without a part of speech keep it empty.
fn split_part_of_speech(line: &str) -> (String, String) {
    if let Some((pos, meaning)) = line.split_once(". ") {
        if !pos.is_empty()
            && pos.len() <= 6
            && pos.chars().all(|c| c.is_ascii_alphabetic() || c == '&')
        {
            return (format!("{pos}."), meaning.trim().to_owned());
        }
    }
    (String::new(), line.trim().to_owned())
}

/// Keeps the first meanings of a long "；"-separated list.
fn shorten(meanings: &str) -> String {
    let parts: Vec<&str> = meanings.split('；').collect();
    if parts.len() <= MAX_MEANINGS {
        meanings.to_owned()
    } else {
        format!("{}……", parts[..MAX_MEANINGS].join("；"))
    }
}

fn parse_youdao(word: &str, body: &str) -> Option<Entry> {
    let root: Value = serde_json::from_str(body).ok()?;
    let entry = &root["ec"]["word"][0];
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let senses: Vec<(String, String)> = entry["trs"]
        .as_array()?
        .iter()
        .filter_map(|tr| text(&tr["tr"][0]["l"]["i"][0]))
        // Lines such as "【名】（Run）（塞）鲁恩（人名）" list people's names, not meanings.
        .filter(|line| !line.starts_with('【'))
        .map(|line| {
            let (pos, meaning) = split_part_of_speech(&line);
            (pos, shorten(&meaning))
        })
        .collect();
    if senses.is_empty() {
        return None;
    }
    let forms = entry["wfs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|form| Some((text(&form["wf"]["name"])?, text(&form["wf"]["value"])?)))
        .collect();
    let examples = root["blng_sents_part"]["sentence-pair"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            Some((
                text(&pair["sentence"])?,
                text(&pair["sentence-translation"])?,
            ))
        })
        .take(MAX_EXAMPLES)
        .collect();
    Some(Entry {
        word: text(&entry["return-phrase"]["l"]["i"]).unwrap_or_else(|| word.to_owned()),
        uk_phonetic: text(&entry["ukphone"]),
        us_phonetic: text(&entry["usphone"]),
        senses,
        forms,
        examples,
    })
}

fn parse_google(word: &str, body: &str) -> Option<Entry> {
    let root: Value = serde_json::from_str(body).ok()?;
    let senses: Vec<(String, String)> = root["dict"]
        .as_array()?
        .iter()
        .filter_map(|group| {
            let terms: Vec<&str> = group["terms"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .take(MAX_MEANINGS)
                .collect();
            let pos = match group["pos"].as_str().unwrap_or_default() {
                "noun" => "n.",
                "verb" => "v.",
                "adjective" => "adj.",
                "adverb" => "adv.",
                "pronoun" => "pron.",
                "preposition" => "prep.",
                "conjunction" => "conj.",
                "interjection" => "int.",
                "abbreviation" => "abbr.",
                _ => "",
            };
            (!terms.is_empty()).then(|| (pos.to_owned(), terms.join("，")))
        })
        .collect();
    (!senses.is_empty()).then(|| Entry {
        word: word.to_owned(),
        senses,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_only_single_english_words() {
        assert!(is_single_english_word("run"));
        assert!(is_single_english_word(" well-known "));
        assert!(is_single_english_word("don't"));
        assert!(!is_single_english_word("hello world"));
        assert!(!is_single_english_word("你好"));
        assert!(!is_single_english_word("1,000"));
        assert!(!is_single_english_word("run."));
        assert!(!is_single_english_word(""));
    }

    #[test]
    fn parses_youdao_entry() {
        let body = r#"{"ec":{"word":[{"usphone":"rʌn","ukphone":"rʌn",
            "return-phrase":{"l":{"i":"run"}},
            "trs":[{"tr":[{"l":{"i":["v. 跑，奔跑；管理"]}}]},
                   {"tr":[{"l":{"i":["n. 跑步"]}}]},
                   {"tr":[{"l":{"i":["【名】 （Run）（塞）鲁恩（人名）"]}}]}],
            "wfs":[{"wf":{"name":"过去式","value":"ran"}}]}]},
            "blng_sents_part":{"sentence-pair":[{"sentence":"Run!","sentence-translation":"跑！"}]}}"#;
        let entry = parse_youdao("run", body).unwrap();
        assert_eq!(entry.us_phonetic.as_deref(), Some("rʌn"));
        assert_eq!(
            entry.senses,
            [
                ("v.".to_owned(), "跑，奔跑；管理".to_owned()),
                ("n.".to_owned(), "跑步".to_owned())
            ]
        );
        assert_eq!(entry.forms, [("过去式".to_owned(), "ran".to_owned())]);
        assert_eq!(entry.examples, [("Run!".to_owned(), "跑！".to_owned())]);
    }

    #[test]
    fn unknown_words_have_no_entry() {
        assert!(parse_youdao("asdfgh", r#"{"typos":{}}"#).is_none());
    }

    #[test]
    fn parses_google_dictionary_with_short_parts_of_speech() {
        let body =
            r#"{"dict":[{"pos":"verb","terms":["运行","跑"]},{"pos":"noun","terms":["跑步"]}]}"#;
        let entry = parse_google("run", body).unwrap();
        assert_eq!(
            entry.senses,
            [
                ("v.".to_owned(), "运行，跑".to_owned()),
                ("n.".to_owned(), "跑步".to_owned())
            ]
        );
    }

    #[test]
    fn shortens_long_meaning_lists() {
        let long = (1..=10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("；");
        assert_eq!(shorten(&long), "1；2；3；4；5；6；7；8……");
        assert_eq!(
            split_part_of_speech("adj. 好的"),
            ("adj.".into(), "好的".into())
        );
        assert_eq!(
            split_part_of_speech("网络 跑"),
            (String::new(), "网络 跑".into())
        );
    }

    #[test]
    #[ignore = "uses Youdao's live dictionary endpoint"]
    fn looks_up_live_word() {
        let entry = lookup("run").unwrap();
        assert!(entry.senses.iter().any(|(pos, _)| pos == "v."));
        println!("{entry:#?}");
    }
}
