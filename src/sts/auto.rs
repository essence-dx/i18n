//! Auto STT with fallback strategy: Whisper offline (primary) -> Google API (disabled)

use crate::error::{I18nError, Result};
use crate::sts::base::SpeechToText;
use crate::sts::google::GoogleSTT;
use async_trait::async_trait;
use std::path::Path;

#[cfg(feature = "whisper")]
use crate::sts::whisper::WhisperSTT;

/// Auto STT with intelligent fallback
///
/// Uses Whisper offline model by default (Google API is currently disabled)
/// Falls back to Google API only if Whisper is unavailable and API key is provided
pub struct AutoSTT {
    _google: Option<GoogleSTT>,
    #[cfg(feature = "whisper")]
    whisper: Option<WhisperSTT>,
}

impl AutoSTT {
    /// Create new Auto STT instance
    pub fn new(_whisper_model_path: Option<impl Into<String>>) -> Result<Self> {
        #[cfg(feature = "whisper")]
        {
            let model_path = _whisper_model_path.map(|p| p.into()).unwrap_or_else(|| {
                let manifest_dir = env!("CARGO_MANIFEST_DIR");
                format!("{}/models/ggml-tiny.en.bin", manifest_dir)
            });

            Ok(Self {
                _google: None,
                whisper: Some(WhisperSTT::new(model_path, Some("en".to_string()))),
            })
        }

        #[cfg(not(feature = "whisper"))]
        {
            Err(I18nError::Other("Whisper feature not enabled".to_string()))
        }
    }

    /// Transcribe audio file (convenience method)
    pub fn transcribe_file(&self, _path: &Path) -> Result<String> {
        #[cfg(feature = "whisper")]
        if let Some(ref whisper) = self.whisper {
            return tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(whisper.transcribe_file(_path));
        }

        Err(I18nError::Other("No STT provider available".to_string()))
    }
}

#[async_trait]
impl SpeechToText for AutoSTT {
    async fn transcribe_file(&self, _path: &Path) -> Result<String> {
        #[cfg(feature = "whisper")]
        if let Some(ref whisper) = self.whisper {
            return whisper.transcribe_file(_path).await;
        }

        Err(I18nError::Other("No STT provider available".to_string()))
    }

    async fn transcribe_samples(&self, _samples: &[f32]) -> Result<String> {
        #[cfg(feature = "whisper")]
        if let Some(ref whisper) = self.whisper {
            return whisper.transcribe_samples(_samples).await;
        }

        Err(I18nError::Other("No STT provider available".to_string()))
    }

    fn get_supported_languages(&self) -> Vec<&'static str> {
        vec!["en"]
    }
}
