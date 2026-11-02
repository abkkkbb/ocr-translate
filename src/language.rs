//! Tells languages apart by their writing system, locally and without a translation service.

/// The writing systems the supported languages are told apart by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Han,
    Kana,
    Hangul,
    Cyrillic,
    Latin,
}

impl Script {
    /// The script of a language given as a Google translation code, such as `zh-CN` or `ja`.
    pub fn of(code: &str) -> Self {
        match code {
            "zh" | "zh-CN" | "zh-TW" => Self::Han,
            "ja" => Self::Kana,
            "ko" => Self::Hangul,
            "ru" => Self::Cyrillic,
            _ => Self::Latin,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Han => "汉字",
            Self::Kana => "日文假名",
            Self::Hangul => "韩文",
            Self::Cyrillic => "西里尔字母",
            Self::Latin => "拉丁字母",
        }
    }
}

/// The script most of `text` is written in, or `None` when it has no letters. A CJK character
/// or Hangul syllable carries about a word, so it weighs as much as a few Latin letters, and
/// Japanese is Han text with a fair share of kana.
pub fn dominant(text: &str) -> Option<Script> {
    let (mut han, mut kana, mut hangul, mut cyrillic, mut latin) = (0, 0, 0, 0, 0);
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9D => kana += 1,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => hangul += 1,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han += 1,
            0x0400..=0x04FF => cyrillic += 1,
            0x0041..=0x005A | 0x0061..=0x007A | 0x00C0..=0x024F if c.is_alphabetic() => latin += 1,
            _ => {}
        }
    }
    let east_asian = if kana > 0 && kana * 4 >= han {
        Script::Kana
    } else {
        Script::Han
    };
    // On a tie the later, non-Latin script wins.
    [
        (Script::Latin, latin),
        (Script::Cyrillic, cyrillic),
        (Script::Hangul, hangul * 3),
        (east_asian, (han + kana) * 3),
    ]
    .into_iter()
    .filter(|&(_, weight)| weight > 0)
    .max_by_key(|&(_, weight)| weight)
    .map(|(script, _)| script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_dominant_script() {
        assert_eq!(dominant("这是一段中文"), Some(Script::Han));
        assert_eq!(dominant("這是繁體中文"), Some(Script::Han));
        assert_eq!(dominant("これは日本語のテキストです"), Some(Script::Kana));
        assert_eq!(
            dominant("日本政府は新しい経済対策を発表した"),
            Some(Script::Kana)
        );
        assert_eq!(dominant("안녕하세요"), Some(Script::Hangul));
        assert_eq!(dominant("Привет, мир"), Some(Script::Cyrillic));
        assert_eq!(dominant("Hello, world"), Some(Script::Latin));
        assert_eq!(dominant("Ça va très bien"), Some(Script::Latin));
        assert_eq!(dominant("123 456 ¥ $ !"), None);
    }

    #[test]
    fn mixed_text_follows_its_main_script() {
        // Chinese with English terms and a Japanese brand name is still Chinese.
        assert_eq!(dominant("我在用 Windows OCR 识别文字"), Some(Script::Han));
        assert_eq!(
            dominant("这款ポケモン游戏在中国大陆很受欢迎，很多玩家都在玩"),
            Some(Script::Han)
        );
        // English with a Chinese name is still English.
        assert_eq!(
            dominant("Welcome back, 张三. Your order has shipped."),
            Some(Script::Latin)
        );
        assert_eq!(dominant("Windowsの設定を開く"), Some(Script::Kana));
    }

    #[test]
    fn maps_translation_codes_to_scripts() {
        assert_eq!(Script::of("zh-CN"), Script::Han);
        assert_eq!(Script::of("zh-TW"), Script::Han);
        assert_eq!(Script::of("ja"), Script::Kana);
        assert_eq!(Script::of("ko"), Script::Hangul);
        assert_eq!(Script::of("ru"), Script::Cyrillic);
        assert_eq!(Script::of("de"), Script::Latin);
    }
}
