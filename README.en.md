# OCR Translate

**English** | [简体中文](README.md)

A lightweight Windows screenshot OCR and translation tool. Press `Ctrl+Shift+T` to select a screen region, then view the recognized source text, translation progress, and result in a floating popup. You can also translate images or typed text from the main window.

[Download for Windows](https://github.com/abkkkbb/ocr-translate/releases/latest) · [All releases](https://github.com/abkkkbb/ocr-translate/releases)

## Features

- **Screen-region translation**: Use the default `Ctrl+Shift+T` shortcut or click **框选屏幕**. A result popup appears as soon as the capture is accepted, and shows OCR text and translation progress as they become available.
- **Latest capture wins**: Start another capture while a translation is running. Once the new capture succeeds, it replaces the previous task; late results from older tasks cannot overwrite it.
- **Multiple floating popups**: Pinned results remain visible across captures. New popups avoid existing windows. Popups can be dragged, resized, pinned, and used to copy the translation or open the main window.
- **Local Windows OCR**: Recognize text with Windows OCR and installed OCR language packs. In regular text-translation modes, only recognized text is sent to the translation provider.
- **Multiple translation providers**: Tencent and Google no-key text translation, Volcengine machine translation (AK/SK), and vision models compatible with OpenAI Chat Completions image input.
- **Image and text translation**: Supports PNG, JPEG, WebP, and BMP images. Translate text entered in the main window, or paste an image into the source pane.
- **Translated image view**: Paint translated text over the capture, compare it with the original, or switch to a text-only view.
- **System tray and lightweight mode**: Hide the app to the tray while keeping the global shortcut active. Optional lightweight mode closes the idle UI child process to release graphics resources.
- **Dictionary cards**: For a single English word translated into Chinese, show phonetics, parts of speech, meanings, word forms, and examples.
- **Appearance and networking**: Day/night themes, font-size controls, and system, direct, HTTP, or SOCKS5 proxy modes.

## Download and run

1. Visit the [latest release](https://github.com/abkkkbb/ocr-translate/releases/latest) and download the Windows `.exe` asset.
2. Run the executable. The Visual C++ Redistributable is not required separately.
3. Press `Ctrl+Shift+T` to select a screen region, or choose an image or enter text in the main window.
4. Configure credentials, service URL, and model in Settings when using Volcengine or a vision provider.

The app targets Windows 10/11. Screen selection requires the built-in Windows Snipping Tool. OCR requires an OCR language pack for the source language. Vision translation requires network access and an OpenAI Chat Completions-compatible service that accepts image input.

## Providers and privacy

- **Tencent / Google**: Use public web translation endpoints for text and may be rate limited, changed, or affected by network conditions. Only OCR-recognized text is sent; OCR itself runs locally.
- **Volcengine**: Uses account AK/SK credentials and the `TranslateText` API. Charges, free quota, and billing access depend on the Volcengine account.
- **Vision models**: The selected image is sent to the model service configured in Settings. Check that service's data and privacy terms before sending sensitive images.
- Settings are stored at `%APPDATA%\ocr-translate\config.json` for the current Windows user. Vision and Volcengine keys are encrypted with Windows DPAPI before being written and can only be decrypted by the same Windows user on the same machine.
- Images are limited to 20 MB. Network requests have timeouts. Temporary captures are stored in the system temp directory and cleaned up by the app.

## Build from source

Requires Windows 10/11, the Rust toolchain, and Visual Studio C++ Build Tools:

```powershell
cargo run
cargo test --locked
cargo build --release --locked
```

The release executable is written to `target/release/ocr-translate.exe`. This is a Windows desktop app and does not run on non-Windows systems.

## Application icon

- `assets/icon.svg`: editable vector source.
- `assets/icon.ico`: multi-size icon embedded in the Windows executable.
- `assets/icon_256.png` and `assets/icon_64.png`: window and tray icon sources.

## Feedback

Please report bugs or suggestions through [GitHub Issues](https://github.com/abkkkbb/ocr-translate/issues).
