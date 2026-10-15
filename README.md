# OCR Translate

A small Windows desktop prototype for recognizing text in an image and showing the source text beside its translation.

## Current prototype

- Choose Tencent or Google as a no-key text translation provider. Windows OCR recognizes the image locally first.
- Choose Volcengine machine translation using an Access Key (AK) and Secret Key (SK) entered in the settings page. It uses Windows OCR first and the signed `TranslateText` API for translation.
- Select an installed Windows OCR language, or let Windows use the user profile languages.
- Configure an OpenAI-compatible vision model as an alternative image translation provider.
- Configure API URL, API key, model, and target language for the vision model.
- Save settings under the current user's application config directory.
- Switch between day and night themes and adjust the app font size in settings; changes preview immediately and persist when settings are saved.
- Select a PNG, JPEG, WebP, or BMP image. Free and Volcengine modes send recognized text to their translation provider; vision mode sends the image to the configured model.
- Show source text and translation in separate editable panes.
- Translate text entered or pasted directly into the source pane with the `翻译原文` button; an image is only required for the OCR workflow.
- Paste an image into the focused source pane with `Ctrl+V` to run the same OCR and translation workflow as selecting an image. Pasting text still inserts text normally; when the clipboard holds both text and a picture (as WPS and Office do when copying text), the text wins.
- Press the global shortcut (default `Ctrl+Shift+T`) while the app is running, or click `框选屏幕`, to select a region with Windows' screen snip UI. The screenshot is copied from the clipboard to a temporary PNG and automatically passed through the existing OCR/translation workflow. With the shortcut, the result appears in a small always-on-top popup next to where the selection ended, without bringing up the main window: it shows the translation, can expand the recognized source text, copy the translation, be pinned, or open the main window. Esc or a click outside closes an unpinned popup. For OCR channels the popup has an image view: Windows OCR word positions are grouped into phrases and paragraphs, each translated on its own, and the translation is painted over the capture in the sampled background and text colours. Numbers and prices are left as they are, hovering a block shows its source text, and `对比原图` toggles the untouched capture. The popup opens in image view when the text is spread over several columns (cards, tables) and in text view otherwise; the custom vision model only has the text view. The `框选屏幕` button still shows the result in the main window. Text or an image that was on the clipboard before the snip is put back afterwards; other clipboard formats (files, rich text formatting) are not preserved.
- Change the global shortcut in settings: Ctrl and/or Alt, optionally Shift, plus a letter, digit, or F1–F12. Pressing it while a capture or translation is still running shows a notice instead of interrupting that work.
- Captures left in the temp folder by a killed process are removed on the next launch once they are more than a day old.
- When the source is a single English word and the target language is Chinese, show a dictionary card with UK/US phonetics, parts of speech with meanings, word forms, and collapsible example sentences, in both the main window and the popup's text view. Details come from Youdao's no-key dictionary endpoint, falling back to Google's dictionary data; this works with any translation channel. A failed lookup only hides the card.
- Reject images larger than 20 MB and stop requests after two minutes.
- Choose a network proxy in settings: follow the Windows system proxy (default, which picks up tools such as Clash in system-proxy mode), connect directly, or use a custom `http://` or `socks5://` address. All translation channels use it.

Windows' `Win+Shift+S` screen snip shortcut and automatic clipboard copy must be available. Closing the app unregisters the shortcut when the process exits; this version does not yet have a system tray.

The no-key providers use website translation endpoints and may be rate limited or change without notice. Google returned HTTP 429 during development; Tencent worked. Only recognized text is sent to those endpoints, while Windows OCR runs locally. If Windows OCR has no language pack for the source text, install one in Windows language settings.

Volcengine is a separate AK/SK-authenticated channel. Your `fire-tran.py` example uses the same `TranslateText` API; any charges or free quota depend on your Volcengine account. Enter the two values in the Volcengine section of the settings page and click `保存设置`. Once configured, that section shows this month's text-translation usage against the 2 million free characters, read from the Volcengine bill detail API (it can lag actual usage by a few hours and covers the whole account), plus the account's available balance. This needs the keys to have read access to the billing center. They are saved in the JSON configuration file under the current user's application config directory (on Windows, normally `%APPDATA%\\ocr-translate\\config.json`). For development, the app also falls back to `VOLCENGINE_ACCESS_KEY` and `VOLCENGINE_SECRET_KEY` when the saved fields are empty. The Volcengine keys and the vision API key are encrypted with Windows DPAPI before being written, so only the same Windows user on the same machine can read them. Keys saved in plain text by earlier versions are encrypted automatically on the next launch. A config file copied to another machine or account cannot be decrypted; re-enter the keys there.

For a packaged desktop application, the settings page plus the per-user JSON file is more practical than shipping a `.env` file. `.env` remains useful for local development, but it is not a secure secret store.

The configured API receives the selected image. Use a vision-capable model and an endpoint compatible with OpenAI Chat Completions image input. The model is asked to return JSON with `source_text` and `translated_text` fields.

## Distribution

`.cargo/config.toml` links the Visual C++ runtime statically, so `target/release/ocr-translate.exe` runs on its own without the VC++ redistributable. It still needs Windows 10 or 11 with an OCR language pack for the source language, the built-in Snipping Tool for the screen-snip shortcut, and an OpenGL-capable display driver.

## Run

```powershell
cargo run
```

Run the response parser checks with:

```powershell
cargo test
```

## Planned implementation order

1. Validate the new screen-selection flow on multi-monitor and mixed-DPI setups, including cancellation.
2. Add tray behavior, hide-to-tray, and reopen-main-window actions.
3. Consider Volcengine's `TranslateImage` endpoint as an optional direct image route.
4. Measure release memory use and reduce startup/runtime costs based on actual measurements.
