#[path = "../src/windows_ocr.rs"]
mod windows_ocr;

fn main() {
    let languages = windows_ocr::available_languages();
    println!("Available Windows OCR languages: {}", languages.join(", "));
    let Some(path) = std::env::args().nth(1) else {
        return;
    };
    let language = std::env::args().nth(2).unwrap_or_else(|| "auto".into());
    match windows_ocr::recognize(std::path::Path::new(&path), &language) {
        Ok(text) => println!("Recognized text:\n{text}"),
        Err(error) => {
            eprintln!("OCR failed: {error}");
            std::process::exit(1);
        }
    }
}
