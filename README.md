# dx-i18n

High-performance internationalization library with translation, text-to-speech (TTS), and speech-to-text (STT) capabilities.

## Features

- **Translation**: Multi-provider translation (Google, DeepL, etc.)
- **Local-first localization**: Existing locale files remain the default workflow
- **Lingo.dev compatibility boundary**: Optional Rust adapter for Lingo.dev-style engines and config
- **Text-to-Speech**: Edge TTS and Google TTS support
- **Speech-to-Text**: Whisper-based offline transcription with embedded tiny.en model (76MB)
- **Fast**: 0.8s transcription on CPU with embedded model
- **Offline**: No external dependencies required for STT

## Quick Start

### Speech-to-Text (STT)

```rust
use dx_i18n::sts::{AutoSTT, SpeechToText};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Uses embedded tiny.en model automatically
    let stt = AutoSTT::new("en-US", None);
    
    let transcript = stt.transcribe_file(Path::new("audio.wav")).await?;
    println!("Transcript: {}", transcript);
    
    Ok(())
}
```

### Text-to-Speech (TTS)

```rust
use dx_i18n::tts::{EdgeTTS, TextToSpeech};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tts = EdgeTTS::new("en-US-AriaNeural");
    
    tts.save("Hello, world!", Path::new("output.mp3")).await?;
    
    Ok(())
}
```

### Translation

```rust
use dx_i18n::locale::{GoogleTranslator, Translator};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let translator = GoogleTranslator::new();
    
    let result = translator.translate("Hello", "en", "es").await?;
    println!("Translation: {}", result);
    
    Ok(())
}
```

## STT Models

The crate includes an embedded **tiny.en** model (76MB) for fast English transcription:

- **Speed**: ~0.8s per 13-second audio on CPU
- **Accuracy**: Suitable for most English transcription tasks
- **No Download**: Model is embedded in the binary

For higher accuracy, you can use custom models:

```rust
let stt = AutoSTT::new("en-US", Some("path/to/ggml-large-v3.bin"));
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
dx-i18n = "0.1"
tokio = { version = "1.0", features = ["full"] }
```

## Examples

See `playgrounds/` directory for more examples:

- `sts_demo.rs` - Speech-to-text with file and microphone input
- `auto_sts_demo.rs` - Auto STT with fallback
- `test_features.rs` - Translation and TTS examples

## Local-first Lingo.dev-compatible localization

`dx_i18n::localization` keeps local localization as the default. Existing JSON or Markdown locale files can be read, protected, hash-tracked, and written without cloud credentials. Lingo.dev-compatible execution is optional and uses config/env boundaries instead of hardcoded secrets.

- Local-only workflows do not require `LINGO_API_KEY` or `LINGODOTDEV_API_KEY`.
- `i18n.json`-style config supports source/target locales, buckets, `include`/`exclude`, locale delimiters, key controls, `injectLocale`, `engineId`, and raw provider config.
- `i18n.lock`-style tracking parses/writes SHA-256 source and key fingerprints to detect current, renamed, changed, and new strings.
- `ProtectedText` preserves placeholders, double-brace interpolation, ICU-style variables, inline code, and fenced code before translation; local render falls back to source text when an existing target string loses protected token, Markdown/table-cell structure, or safe identity for repeated same-kind Markdown units.
- `LingoApiProvider` targets the current `POST /process/localize` API shape, validates response locale/data shape on the safe path, preserves optional response metadata, applies bounded HTTP timeouts, bounds successful response bodies before JSON parsing, bounds and redacts error response bodies, and keeps API keys out of request JSON.
- `LocalizationWorkspace` and the `dx-i18n` binary provide a local-first command boundary for `show config`, `status`, `lockfile`, `lockfile --check`, and JSON/Markdown `run`.
- `status` is auth-free and read-only. Use repeated `--locale <locale>` to narrow target locales, repeated `--bucket <type>` to narrow configured bucket types, and repeated `--file <substring>` to narrow expanded include/source/target paths in the report. It reports lockfile-pending source units and target output drift separately, so a current `i18n.lock` can still reveal stale target files before `run --frozen` fails. `status --verbose` prints pending source-file/key detail and drifted target file paths without reading API key env vars, contacting providers, or writing files. `status --force` bypasses lockfile change detection for retranslation estimates while still respecting locked, preserved, ignored, and injected key controls.
- Plain `run` is local by default and processes all configured target locales, then writes a fresh `i18n.lock` after a successful non-frozen unfiltered run. Use repeated `--target-locale <locale>` or `--target <locale>` to narrow locales, `--bucket <type>` to narrow configured bucket types, `--file <substring>` to narrow expanded bucket paths, and `--key <dot.prefix>` to narrow key prefixes. Filtered non-frozen runs fail before writing when file filters select no source files or key filters select no localization units. Successful filtered runs merge selected source fingerprints into the existing lockfile while replacing stale entries for the same selected keys and preserving current unselected same-key entries. Requested targets and filters are validated before output writes. `run --frozen` is a no-write verification gate over the selected scope: it refuses stale selected source lock state and target output drift. Local `run --force` remains auth-free and rewrites from local sources/targets; `--force` cannot be combined with `--frozen`. `--lingo` or `--mode lingo` explicitly opts into the Lingo.dev-compatible HTTP adapter and requires Lingo credentials from env or the explicit non-empty `--api-key <api-key>` override. Lingo mode sends only lockfile-pending provider-eligible units by default, skips provider requests when nothing is pending, and leaves target files plus `i18n.lock` untouched when no provider translations pass DX safety checks; filtered Lingo runs update lock fingerprints only for provider-accepted keys. Use `--force` to re-send all selected provider-eligible units while still writing only provider-accepted changes and respecting locked, preserved, ignored, and injected key controls.
- `--provider openai|anthropic|google|mistral|openrouter|ollama` is recognized as the Rust-side raw-provider boundary for Lingo.dev CLI compatibility, but direct raw-provider execution is not implemented yet and exits before writing files.
- `lockfile --check` compares parsed lock state rather than byte-for-byte YAML formatting. Plain `lockfile` creates a missing lockfile or reports an existing current lockfile; use `lockfile --force` for an explicit rewrite.
- Status reports Lingo-supported bucket types that this Rust workspace has not implemented yet, such as YAML, instead of silently presenting them as processed. Unfiltered `run` and `lockfile` block on unsupported buckets; `run --bucket json` can safely skip an unselected unsupported bucket.

See `docs/LINGO_DEV_COMPAT.md` for the integration model and environment variables.

## License

Licensed under MIT or Apache-2.0.
