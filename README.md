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
- Press the global shortcut (default `Ctrl+Shift+T`) while the app is running, or click `框选屏幕`, to select a region with Windows' screen snip UI. The screenshot is copied from the clipboard to a temporary PNG and automatically passed through the existing OCR/translation workflow. With the shortcut, the result appears in a small always-on-top popup next to where the selection ended, without bringing up the main window: it shows the translation, can expand the recognized source text, copy the translation, be pinned, or open the main window. Esc or a click outside closes an unpinned popup. All image translation channels support an image view with `图片` / `文字` switching and `对比原图`. For OCR channels, Windows OCR word positions are grouped into phrases and paragraphs, each translated on its own, and the translation is painted over the capture in the sampled background and text colours. Numbers and prices are left as they are, and hovering a block shows its source text. OCR results open in image view when text is spread over several columns (cards, tables) and in text view otherwise. Custom vision results open in image view: one multimodal request asks for both the full translation and translated blocks, using local OCR ids as optional position hints and normalized model boxes for other text. No OCR language pack is required for the vision route. If the model only returns full text or omits valid positions, the image view shows the original capture with the full translation below it. The `框选屏幕` button still shows the result in the main window. Text or an image that was on the clipboard before the snip is put back afterwards; other clipboard formats (files, rich text formatting) are not preserved.
- Change the global shortcut in settings: Ctrl and/or Alt, optionally Shift, plus a letter, digit, or F1–F12. Pressing it while a capture or translation is still running shows a notice instead of interrupting that work.
- Captures left in the temp folder by a killed process are removed on the next launch once they are more than a day old.
- Basic memory optimizations are always enabled: screenshot textures are created only in the popup's image view, at its physical display resolution, and released on switching to text or closing the popup. Full screenshot pixels and block layouts are released once no popup or translation task needs them; source text, translation, dictionary details and the current image file remain available for viewing and retranslation. Hidden or minimized main windows skip drawing their controls, and pinned popups do not poll for outside clicks.
- The tray menu offers an optional lightweight mode, off by default and reset on each launch. It exits the UI child process when idle with no popup, releasing its graphics and font resources while the resident process keeps the tray, global shortcut and translation tasks available. Both processes use the same portable exe. Capture results open the UI on demand; reopening can take longer. Active captures, translations and pinned popups defer release. Opening the main window or settings leaves lightweight mode, with the last source text, translation and selected image file preserved.
- When the source is a single English word and the target language is Chinese, show a dictionary card with UK/US phonetics, parts of speech with meanings, word forms, and collapsible example sentences, in both the main window and the popup's text view. Details come from Youdao's no-key dictionary endpoint, falling back to Google's dictionary data; this works with any translation channel. A failed lookup only hides the card.
- Reject images larger than 20 MB and stop requests after two minutes.
- Choose a network proxy in settings: follow the Windows system proxy (default, which picks up tools such as Clash in system-proxy mode), connect directly, or use a custom `http://` or `socks5://` address. All translation channels use it.

Windows' Snipping Tool (`ms-screenclip:` protocol) and automatic clipboard copy must be available. The selector is launched directly through the protocol; a failure to open it is reported after ten seconds. Background capture and translation completion wake the app even when its main window is hidden or minimized. Closing the main window follows the selected close behavior; hiding it in the system tray removes its taskbar button while keeping the global shortcut available. Double-click the tray icon to reopen the main window, or right-click it to open the main window, capture a region, open settings, or exit. Exiting unregisters the shortcut. If tray initialization fails, closing the window exits normally. The portable exe includes its own application icon.

The no-key providers use website translation endpoints and may be rate limited or change without notice. Google returned HTTP 429 during development; Tencent worked. Only recognized text is sent to those endpoints, while Windows OCR runs locally. If Windows OCR has no language pack for the source text, install one in Windows language settings.

Volcengine is a separate AK/SK-authenticated channel. Your `fire-tran.py` example uses the same `TranslateText` API; any charges or free quota depend on your Volcengine account. Enter the two values in the Volcengine section of the settings page and click `保存设置`. Once configured, that section shows this month's text-translation usage against the 2 million free characters, read from the Volcengine bill detail API (it can lag actual usage by a few hours and covers the whole account), plus the account's available balance. This needs the keys to have read access to the billing center. They are saved in the JSON configuration file under the current user's application config directory (on Windows, normally `%APPDATA%\\ocr-translate\\config.json`). For development, the app also falls back to `VOLCENGINE_ACCESS_KEY` and `VOLCENGINE_SECRET_KEY` when the saved fields are empty. The Volcengine keys and the vision API key are encrypted with Windows DPAPI before being written, so only the same Windows user on the same machine can read them. Keys saved in plain text by earlier versions are encrypted automatically on the next launch. A config file copied to another machine or account cannot be decrypted; re-enter the keys there.

For a packaged desktop application, the settings page plus the per-user JSON file is more practical than shipping a `.env` file. `.env` remains useful for local development, but it is not a secure secret store.

The configured API receives the selected image. Use a vision-capable model and an endpoint compatible with OpenAI Chat Completions image input. The model is asked to return JSON with `source_text` and `translated_text` fields.

## Distribution

Only one instance runs per Windows session. Launching the portable exe again restores the existing main window, including when it is hidden in the system tray, without creating another tray icon or registering another hotkey.

Settings offer two close behaviors: Hide to tray (default) and Exit directly. Closing the main window immediately follows this setting without a dialog; click Save settings to persist changes. The tray's Exit command always exits immediately. If tray initialization fails, closing the main window exits normally.

`.cargo/config.toml` links the Visual C++ runtime statically, so `target/release/ocr-translate.exe` runs on its own without the VC++ redistributable. It still needs Windows 10 or 11 with an OCR language pack for the source language, the built-in Snipping Tool for the screen-snip shortcut, and an OpenGL-capable display driver.

## Application icon

The application mark combines mint screenshot-selection corners with white `文` / `A` language glyphs on a navy-to-petrol rounded tile. The 16–24 px variants simplify the lettering for legibility; the tray uses the same simplified mark rendered at 64 px for DPI scaling.

- `assets/icon.svg`: editable, font-independent vector master.
- `assets/icon.ico`: 13 individually rendered Windows sizes from 16 to 256 px, embedded in the portable exe by `build.rs`.
- `assets/icon_256.png`: main-window and taskbar icon.
- `assets/icon_64.png`: small-size-optimized tray icon source.
- `assets/icon_preview.png`: before/after comparison and actual-size samples on light/dark backgrounds. These are design previews, not screenshots of the running app.

Regenerate the assets with Python and Pillow, then rebuild the exe:

```powershell
python tools_make_icon.py
cargo build --release
```

The generator verifies the ICO directory, exact per-size artwork and transparent corners. Windows Explorer may cache the old exe/shortcut icon; restarting the app updates its runtime icons, while an Explorer refresh or a new shortcut may be needed for the file icon.

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
2. Consider Volcengine's `TranslateImage` endpoint as an optional direct image route.
3. Measure release memory use and reduce startup/runtime costs based on actual measurements.
