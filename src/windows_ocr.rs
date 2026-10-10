use std::path::Path;
use windows::{
    core::HSTRING,
    Globalization::Language,
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode,
    },
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
};

pub fn available_languages() -> Vec<String> {
    let Ok(languages) = OcrEngine::AvailableRecognizerLanguages() else {
        return Vec::new();
    };
    let Ok(count) = languages.Size() else {
        return Vec::new();
    };
    (0..count)
        .filter_map(|index| languages.GetAt(index).ok())
        .filter_map(|language| language.LanguageTag().ok())
        .map(|tag| tag.to_string())
        .collect()
}

pub fn recognize(path: &Path, language_tag: &str) -> Result<String, String> {
    let engine = if language_tag == "auto" {
        OcrEngine::TryCreateFromUserProfileLanguages()
    } else {
        let language = Language::CreateLanguage(&HSTRING::from(language_tag))
            .map_err(|error| format!("OCR 语言无效：{error}"))?;
        OcrEngine::TryCreateFromLanguage(&language)
    }
    .map_err(|error| format!("无法创建 Windows OCR：{error}"))?;

    let file_path = path.to_str().ok_or("图片路径无法转换为 Unicode。")?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(file_path))
        .and_then(|operation| operation.get())
        .map_err(|error| format!("Windows 无法打开图片：{error}"))?;
    let stream = file
        .OpenReadAsync()
        .and_then(|operation| operation.get())
        .map_err(|error| format!("读取图片失败：{error}"))?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .and_then(|operation| operation.get())
        .map_err(|error| format!("Windows 无法解码图片：{error}"))?;

    let width = decoder.PixelWidth().map_err(|error| error.to_string())?;
    let height = decoder.PixelHeight().map_err(|error| error.to_string())?;
    let max = OcrEngine::MaxImageDimension().map_err(|error| error.to_string())?;
    let bitmap = if width.max(height) > max {
        let scale = max as f64 / width.max(height) as f64;
        let transform = BitmapTransform::new().map_err(|error| error.to_string())?;
        transform
            .SetScaledWidth(((width as f64 * scale).round() as u32).max(1))
            .map_err(|error| error.to_string())?;
        transform
            .SetScaledHeight(((height as f64 * scale).round() as u32).max(1))
            .map_err(|error| error.to_string())?;
        decoder.GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
    } else {
        decoder.GetSoftwareBitmapConvertedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
        )
    }
    .and_then(|operation| operation.get())
    .map_err(|error| format!("准备 OCR 图片失败：{error}"))?;

    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|operation| operation.get())
        .map_err(|error| format!("Windows OCR 识别失败：{error}"))?;
    let text = result
        .Text()
        .map_err(|error| format!("读取 OCR 结果失败：{error}"))?
        .to_string();
    let text = normalize_cjk_spacing(&text);
    if text.trim().is_empty() {
        Err("没有识别到文字。请换一张清晰图片，或切换原文识别语言。".into())
    } else {
        Ok(text)
    }
}

fn normalize_cjk_spacing(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut normalized = String::with_capacity(text.len());
    for (index, character) in chars.iter().enumerate() {
        if *character == ' '
            && index > 0
            && is_han(chars[index - 1])
            && chars[index + 1..]
                .iter()
                .find(|next| **next != ' ')
                .is_some_and(|next| is_han(*next))
        {
            continue;
        }
        normalized.push(*character);
    }
    normalized
}

fn is_han(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_windows_ocr_spaces_between_han_characters() {
        assert_eq!(
            normalize_cjk_spacing("你 好 世 界\nHello world"),
            "你好世界\nHello world"
        );
    }
}
