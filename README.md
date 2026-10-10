# OCR Translate

A small Windows desktop prototype for recognizing text in an image and showing the source text beside its translation.

## Current prototype

- Choose Tencent or Google as a no-key text translation provider. Windows OCR recognizes the image locally first.
- Choose Volcengine machine translation using an Access Key (AK) and Secret Key (SK) entered in the settings page. It uses Windows OCR first and the signed `TranslateText` API for translation.
- Select an installed Windows OCR language, or let Windows use the user profile languages.
- Configure an OpenAI-compatible vision model as an alternative image translation provider.
- Configure API URL, API key, model, and target language for the vision model.
- Save settings under the current user's application config directory.
- Select a PNG, JPEG, WebP, or BMP image. Free and Volcengine modes send recognized text to their translation provider; vision mode sends the image to the configured model.
- Show source text and translation in separate editable panes.
- Translate text entered or pasted directly into the source pane with the `翻译原文` button; an image is only required for the OCR workflow.
- Reject images larger than 20 MB and stop requests after two minutes.

This version does not yet include global hotkeys, screen-region capture, or a system tray.

The no-key providers use website translation endpoints and may be rate limited or change without notice. Google returned HTTP 429 during development; Tencent worked. Only recognized text is sent to those endpoints, while Windows OCR runs locally. If Windows OCR has no language pack for the source text, install one in Windows language settings.

Volcengine is a separate AK/SK-authenticated channel. Your `fire-tran.py` example uses the same `TranslateText` API; any charges or free quota depend on your Volcengine account. Enter the two values in the Volcengine section of the settings page and click `保存设置`. They are saved in the JSON configuration file under the current user's application config directory (on Windows, normally `%APPDATA%\\ocr-translate\\config.json`). For development, the app also falls back to `VOLCENGINE_ACCESS_KEY` and `VOLCENGINE_SECRET_KEY` when the saved fields are empty. The JSON file currently stores these credentials as plain text; protect the file and do not commit or share it.

For a packaged desktop application, the settings page plus the per-user JSON file is more practical than shipping a `.env` file. `.env` remains useful for local development, but it is not a secure secret store. A future release can protect saved credentials with Windows DPAPI or Credential Manager.

The configured API receives the selected image. Use a vision-capable model and an endpoint compatible with OpenAI Chat Completions image input. The model is asked to return JSON with `source_text` and `translated_text` fields.

## Run

```powershell
cargo run
```

Run the response parser checks with:

```powershell
cargo test
```

## Planned implementation order

1. Add global shortcut registration and a screen-region selector.
2. Add multi-monitor capture and pass the selected crop to the existing request path.
3. Add tray behavior, hide-to-tray, and reopen-main-window actions.
4. Consider Volcengine's `TranslateImage` endpoint as an optional direct image route.
5. Measure release memory use and reduce startup/runtime costs based on actual measurements.
