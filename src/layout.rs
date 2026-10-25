//! Groups positioned OCR words into translatable blocks and samples their colours, so a
//! translation can be painted back over the screenshot where the source text was.

use crate::windows_ocr::Word;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A model can refer to an OCR block by id, or locate text itself on a 0..1000 grid.
#[derive(Debug, Deserialize)]
pub struct VisionBlock {
    #[serde(default)]
    pub id: Option<usize>,
    #[serde(default)]
    pub bbox: Option<[f32; 4]>,
    #[serde(default, alias = "original")]
    pub source_text: String,
    #[serde(default, alias = "translation")]
    pub translated_text: String,
    #[serde(default)]
    pub line_count: Option<u32>,
}

/// A run of text translated as one unit: a phrase, or a paragraph of wrapped lines.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextBlock {
    /// `[x, y, width, height]` in image pixels.
    pub rect: [f32; 4],
    pub line_height: f32,
    pub source: String,
    /// `None` for blocks left as they are, such as numbers, prices and stray symbols.
    pub translated: Option<String>,
    pub background: [u8; 3],
    pub foreground: [u8; 3],
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ImageLayout {
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub rgba: Vec<u8>,
    pub blocks: Vec<TextBlock>,
    /// Text spread over several columns, like cards or tables, reads better as an image.
    pub dispersed: bool,
}

impl ImageLayout {
    pub fn build(path: &Path, lines: Vec<Vec<Word>>) -> Result<Self, String> {
        let image = image::open(path)
            .map_err(|error| format!("读取截图失败：{error}"))?
            .to_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        let mut blocks = group_blocks(lines);
        for block in &mut blocks {
            let (background, foreground) = sample_colors(&rgba, width, height, block.rect);
            block.background = background;
            block.foreground = foreground;
        }
        let dispersed = is_dispersed(&blocks);
        Ok(Self {
            width,
            height,
            rgba,
            blocks,
            dispersed,
        })
    }

    pub fn source_text(&self) -> String {
        join_lines(self.blocks.iter().map(|block| block.source.as_str()))
    }

    pub fn apply_vision_blocks(&mut self, blocks: impl IntoIterator<Item = VisionBlock>) {
        let reference_count = self.blocks.len();
        for block in blocks {
            let translated = block.translated_text.trim();
            if translated.is_empty() {
                continue;
            }
            if let Some(index) = block.id.filter(|&index| index < reference_count) {
                let reference = &mut self.blocks[index];
                // Duplicate ids must not overwrite an already mapped translation.
                if reference.translated.is_none() {
                    reference.translated = Some(translated.into());
                    if !block.source_text.trim().is_empty() {
                        reference.source = block.source_text.trim().into();
                    }
                }
                continue;
            }
            let Some(rect) = block.bbox.and_then(|bbox| self.model_rect(bbox)) else {
                continue;
            };
            let (background, foreground) = sample_colors(&self.rgba, self.width, self.height, rect);
            self.blocks.push(TextBlock {
                rect,
                line_height: (rect[3] / block.line_count.unwrap_or(1).clamp(1, 100) as f32)
                    .max(1.0),
                source: block.source_text.trim().into(),
                translated: Some(translated.into()),
                background,
                foreground,
            });
        }
        self.dispersed = is_dispersed(&self.blocks);
    }

    fn model_rect(&self, bbox: [f32; 4]) -> Option<[f32; 4]> {
        let [x, y, width, height] = bbox;
        if !bbox.iter().all(|value| value.is_finite()) || width <= 0.0 || height <= 0.0 {
            return None;
        }
        let left = x.clamp(0.0, 1000.0);
        let top = y.clamp(0.0, 1000.0);
        let right = (x + width).clamp(0.0, 1000.0);
        let bottom = (y + height).clamp(0.0, 1000.0);
        if right <= left || bottom <= top {
            return None;
        }
        let sx = self.width as f32 / 1000.0;
        let sy = self.height as f32 / 1000.0;
        Some([
            left * sx,
            top * sy,
            (right - left) * sx,
            (bottom - top) * sy,
        ])
    }

    /// Indexes of blocks worth sending to a translator.
    pub fn translatable(&self) -> Vec<usize> {
        (0..self.blocks.len())
            .filter(|&index| needs_translation(&self.blocks[index].source))
            .collect()
    }

    pub fn translated_text(&self) -> String {
        join_lines(
            self.blocks
                .iter()
                .map(|block| block.translated.as_deref().unwrap_or(block.source.as_str())),
        )
    }
}

fn join_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    lines.collect::<Vec<_>>().join("\n")
}

/// Numbers, prices and single stray letters (often radio buttons or icons) stay as they are.
fn needs_translation(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphabetic()).count() >= 2
}

/// Scripts written without spaces between words.
fn is_unspaced(character: char) -> bool {
    matches!(character,
        '\u{3000}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'
        | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}')
}

/// `tight` joins pieces of one word that OCR split apart.
fn append_word(text: &mut String, word: &str, tight: bool) {
    let joins_tight = tight
        || text.chars().last().is_some_and(is_unspaced)
            && word.chars().next().is_some_and(is_unspaced);
    if !text.is_empty() && !joins_tight {
        text.push(' ');
    }
    text.push_str(word);
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let left = a[0].min(b[0]);
    let top = a[1].min(b[1]);
    let right = (a[0] + a[2]).max(b[0] + b[2]);
    let bottom = (a[1] + a[3]).max(b[1] + b[3]);
    [left, top, right - left, bottom - top]
}

/// Splits each OCR line at wide gaps, then merges vertically adjacent, left-aligned segments of
/// similar height into paragraphs. Blocks come back in reading order.
fn group_blocks(lines: Vec<Vec<Word>>) -> Vec<TextBlock> {
    let mut segments: Vec<TextBlock> = Vec::new();
    for mut line in lines {
        line.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        let height = line.iter().map(|word| word.rect[3]).fold(0.0, f32::max);
        let mut current: Option<TextBlock> = None;
        for word in line {
            if let Some(segment) = current.as_mut() {
                let gap = word.rect[0] - (segment.rect[0] + segment.rect[2]);
                if gap <= 1.5 * height {
                    append_word(&mut segment.source, &word.text, gap < 0.3 * height);
                    segment.rect = union(segment.rect, word.rect);
                    continue;
                }
                segments.extend(current.take());
            }
            current = Some(TextBlock {
                rect: word.rect,
                line_height: height,
                source: word.text,
                translated: None,
                background: [255; 3],
                foreground: [0; 3],
            });
        }
        segments.extend(current);
    }
    segments.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));

    let mut blocks: Vec<TextBlock> = Vec::new();
    for segment in segments {
        let height = segment.line_height;
        let continues = blocks.iter_mut().rev().find(|block| {
            let gap = segment.rect[1] - (block.rect[1] + block.rect[3]);
            let ratio = height / block.line_height;
            let overlaps = segment.rect[0] < block.rect[0] + block.rect[2]
                && block.rect[0] < segment.rect[0] + segment.rect[2];
            (-0.25 * height..=0.6 * height).contains(&gap)
                && (segment.rect[0] - block.rect[0]).abs() <= 2.0 * height
                && (0.7..=1.4).contains(&ratio)
                && overlaps
        });
        match continues {
            Some(block) => {
                append_word(&mut block.source, &segment.source, false);
                block.rect = union(block.rect, segment.rect);
            }
            None => blocks.push(segment),
        }
    }
    // Rows first, left to right within a row.
    blocks.sort_by(|a, b| {
        let same_row = (a.rect[1] - b.rect[1]).abs() < 0.5 * a.line_height.min(b.line_height);
        if same_row {
            a.rect[0].total_cmp(&b.rect[0])
        } else {
            a.rect[1].total_cmp(&b.rect[1])
        }
    });
    blocks
}

/// True when translatable blocks start in two or more columns that each hold several blocks.
fn is_dispersed(blocks: &[TextBlock]) -> bool {
    let text_blocks: Vec<&TextBlock> = blocks
        .iter()
        .filter(|block| needs_translation(&block.source))
        .collect();
    if text_blocks.len() < 3 {
        return false;
    }
    let mut heights: Vec<f32> = text_blocks.iter().map(|block| block.line_height).collect();
    heights.sort_by(f32::total_cmp);
    let tolerance = 3.0 * heights[heights.len() / 2];
    let mut lefts: Vec<f32> = text_blocks.iter().map(|block| block.rect[0]).collect();
    lefts.sort_by(f32::total_cmp);
    let mut columns = Vec::new();
    let mut size = 1;
    for pair in lefts.windows(2) {
        if pair[1] - pair[0] > tolerance {
            columns.push(size);
            size = 0;
        }
        size += 1;
    }
    columns.push(size);
    columns.iter().filter(|&&size| size >= 2).count() >= 2
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let index = ((y * width + x) * 4) as usize;
    [rgba[index], rgba[index + 1], rgba[index + 2]]
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (*x as i32 - y as i32).unsigned_abs())
        .sum()
}

/// The most common colour among `pixels`, bucketed so anti-aliasing noise does not split it.
fn dominant(pixels: impl Iterator<Item = [u8; 3]>) -> Option<[u8; 3]> {
    let mut buckets: std::collections::HashMap<[u8; 3], (u32, [u32; 3])> = Default::default();
    for color in pixels {
        let entry = buckets.entry(color.map(|c| c >> 4)).or_default();
        entry.0 += 1;
        for (sum, channel) in entry.1.iter_mut().zip(color) {
            *sum += u32::from(channel);
        }
    }
    buckets
        .into_values()
        .max_by_key(|(count, _)| *count)
        .map(|(count, sums)| sums.map(|sum| (sum / count) as u8))
}

/// Background from the ring just outside the box; text colour from pixels inside it that stand
/// out from that background.
fn sample_colors(rgba: &[u8], width: u32, height: u32, rect: [f32; 4]) -> ([u8; 3], [u8; 3]) {
    if width == 0 || height == 0 {
        return ([255; 3], [0; 3]);
    }
    let clamp_x = |value: f32| (value.max(0.0) as u32).min(width - 1);
    let clamp_y = |value: f32| (value.max(0.0) as u32).min(height - 1);
    let (left, top) = (clamp_x(rect[0] - 3.0), clamp_y(rect[1] - 3.0));
    let (right, bottom) = (
        clamp_x(rect[0] + rect[2] + 3.0),
        clamp_y(rect[1] + rect[3] + 3.0),
    );
    let ring = (left..=right)
        .flat_map(|x| [(x, top), (x, bottom)])
        .chain((top..=bottom).flat_map(|y| [(left, y), (right, y)]))
        .map(|(x, y)| pixel(rgba, width, x, y));
    let background = dominant(ring).unwrap_or([255; 3]);

    let inside = (clamp_y(rect[1])..=clamp_y(rect[1] + rect[3]))
        .flat_map(|y| (clamp_x(rect[0])..=clamp_x(rect[0] + rect[2])).map(move |x| (x, y)))
        .map(|(x, y)| pixel(rgba, width, x, y))
        .filter(|color| distance(*color, background) > 120);
    let foreground = dominant(inside).unwrap_or_else(|| {
        let luminance = background.iter().map(|c| u32::from(*c)).sum::<u32>();
        if luminance > 384 {
            [0; 3]
        } else {
            [255; 3]
        }
    });
    (background, foreground)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_page() -> ImageLayout {
        ImageLayout {
            width: 200,
            height: 100,
            rgba: [250, 250, 250, 255].repeat(200 * 100),
            blocks: Vec::new(),
            dispersed: false,
        }
    }

    #[test]
    fn layout_loads_every_selectable_image_format() {
        for extension in ["png", "jpg", "webp", "bmp"] {
            let path = std::env::temp_dir().join(format!(
                "ocr-layout-format-test-{}.{extension}",
                std::process::id()
            ));
            image::RgbImage::from_pixel(20, 10, image::Rgb([240, 240, 240]))
                .save(&path)
                .unwrap();
            let result = ImageLayout::build(&path, Vec::new());
            let _ = std::fs::remove_file(path);
            let page = result.unwrap();
            assert_eq!((page.width, page.height), (20, 10));
        }
    }

    #[test]
    fn vision_boxes_scale_to_image_pixels_and_clip_at_edges() {
        let mut page = blank_page();
        let blocks: Vec<VisionBlock> = serde_json::from_value(serde_json::json!([
            {"bbox": [100, 200, 400, 300], "source_text": "Hello", "translated_text": "你好", "line_count": 2},
            {"bbox": [900, 900, 200, 200], "translated_text": "边缘"},
            {"bbox": [1001, 0, 20, 20], "translated_text": "外部"},
            {"bbox": [0, 0, -20, 20], "translated_text": "无效"},
            {"bbox": [0, 0, 20, 20], "translated_text": " "}
        ])).unwrap();
        page.apply_vision_blocks(blocks);
        assert_eq!(page.blocks.len(), 2);
        assert_eq!(page.blocks[0].rect, [20.0, 20.0, 80.0, 30.0]);
        assert_eq!(page.blocks[0].line_height, 15.0);
        assert_eq!(page.blocks[0].translated.as_deref(), Some("你好"));
        assert_eq!(page.blocks[0].background, [250; 3]);
        assert_eq!(page.blocks[1].rect, [180.0, 90.0, 20.0, 10.0]);
        assert!(page.model_rect([f32::NAN, 0.0, 10.0, 10.0]).is_none());
    }

    #[test]
    fn vision_ids_preserve_ocr_positions_and_ignore_duplicate_ids() {
        let mut page = blank_page();
        page.blocks = group_blocks(vec![vec![word("He11o", 10.0, 10.0, 50.0)]]);
        let original_rect = page.blocks[0].rect;
        let blocks: Vec<VisionBlock> = serde_json::from_value(serde_json::json!([
            {"id": 0, "bbox": [0, 0, 1000, 1000], "source_text": "Hello", "translated_text": "你好"},
            {"id": 0, "translated_text": "重复"},
            {"id": 5, "translated_text": "未知"}
        ])).unwrap();
        page.apply_vision_blocks(blocks);
        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.blocks[0].rect, original_rect);
        assert_eq!(page.blocks[0].source, "Hello");
        assert_eq!(page.blocks[0].translated.as_deref(), Some("你好"));
    }

    fn word(text: &str, x: f32, y: f32, width: f32) -> Word {
        Word {
            text: text.into(),
            rect: [x, y, width, 13.0],
        }
    }

    #[test]
    fn splits_lines_at_wide_gaps() {
        let blocks = group_blocks(vec![vec![
            word("Searches", 116.0, 426.0, 66.0),
            word("/", 187.0, 426.0, 7.0),
            word("month", 200.0, 426.0, 49.0),
            word("1,000", 466.0, 426.0, 43.0),
        ]]);
        let sources: Vec<&str> = blocks.iter().map(|b| b.source.as_str()).collect();
        assert_eq!(sources, ["Searches / month", "1,000"]);
    }

    #[test]
    fn rejoins_words_split_by_ocr() {
        let blocks = group_blocks(vec![vec![
            word("M", 271.0, 10.0, 12.0),
            word("ax", 286.0, 10.0, 16.0),
        ]]);
        assert_eq!(blocks[0].source, "Max");
    }

    #[test]
    fn merges_wrapped_lines_into_a_paragraph_but_not_spaced_rows() {
        let blocks = group_blocks(vec![
            vec![
                word("The", 10.0, 10.0, 30.0),
                word("quick", 44.0, 10.0, 40.0),
            ],
            vec![word("fox", 10.0, 27.0, 30.0)],
            vec![word("Next", 10.0, 70.0, 30.0)],
        ]);
        let sources: Vec<&str> = blocks.iter().map(|b| b.source.as_str()).collect();
        assert_eq!(sources, ["The quick fox", "Next"]);
    }

    #[test]
    fn joins_chinese_without_spaces() {
        let mut text = String::from("你");
        append_word(&mut text, "好", false);
        append_word(&mut text, "OK", false);
        assert_eq!(text, "你好 OK");
    }

    #[test]
    fn leaves_numbers_and_stray_letters_untranslated() {
        assert!(!needs_translation("$25"));
        assert!(!needs_translation("1,000"));
        assert!(!needs_translation("o"));
        assert!(needs_translation("+ $25 / month"));
    }

    #[test]
    fn detects_card_layouts_but_not_single_column_prose() {
        let card = |x: f32, y: f32| group_blocks(vec![vec![word("Searches month", x, y, 100.0)]]);
        let cards: Vec<TextBlock> = [(100.0, 10.0), (100.0, 60.0), (600.0, 10.0), (600.0, 60.0)]
            .iter()
            .flat_map(|(x, y)| card(*x, *y))
            .collect();
        assert!(is_dispersed(&cards));
        let prose: Vec<TextBlock> = [10.0, 60.0, 110.0]
            .iter()
            .flat_map(|y| card(100.0, *y))
            .collect();
        assert!(!is_dispersed(&prose));
    }

    #[test]
    fn samples_background_and_text_colours() {
        let (width, height) = (20, 10);
        let mut rgba = [250u8, 250, 250, 255].repeat(width * height);
        for x in 8..12 {
            let index = (5 * width + x) * 4;
            rgba[index..index + 3].copy_from_slice(&[20, 30, 40]);
        }
        let (background, foreground) =
            sample_colors(&rgba, width as u32, height as u32, [5.0, 3.0, 10.0, 4.0]);
        assert_eq!(background, [250; 3]);
        assert_eq!(foreground, [20, 30, 40]);
    }
}
