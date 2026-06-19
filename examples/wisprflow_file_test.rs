//! Test Wispr Flow with existing audio file

#[cfg(all(feature = "whisper", feature = "wisprflow"))]
use dx_i18n::{sts::AutoSTT, wisprflow::WisprFlow};
#[cfg(all(feature = "whisper", feature = "wisprflow"))]
use std::path::PathBuf;
#[cfg(all(feature = "whisper", feature = "wisprflow"))]
use std::time::Instant;

#[cfg(all(feature = "whisper", feature = "wisprflow"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🎙️  Wispr Flow - File Test\n");

    let audio_path = PathBuf::from("F:/Code/dx/crates/i18n/audio.wav");

    if !audio_path.exists() {
        eprintln!("❌ Audio file not found: {:?}", audio_path);
        return Ok(());
    }

    println!("📁 Testing with: {:?}\n", audio_path);

    let start_total = Instant::now();

    // Step 1: Speech-to-text with Whisper
    println!("🔊 Transcribing with Whisper tiny.en...");
    let stt_start = Instant::now();

    let stt = AutoSTT::new(None::<String>)?;
    let raw_transcript = stt.transcribe_file(&audio_path)?;

    let stt_time = stt_start.elapsed();
    println!("✅ Transcription complete: {:.2}s", stt_time.as_secs_f64());
    println!("📝 Raw: \"{}\"\n", raw_transcript);

    // Step 2: Text enhancement with Rust_Grammar
    println!("✨ Enhancing with Rust_Grammar...");
    let enhance_start = Instant::now();

    let flow = WisprFlow::new()?;
    let result = flow.process_text(&raw_transcript)?;

    let enhance_time = enhance_start.elapsed();
    println!(
        "✅ Enhancement complete: {:.2}s",
        enhance_time.as_secs_f64()
    );
    println!("📝 Enhanced: \"{}\"\n", result.enhanced_text);

    // Results
    let total_time = start_total.elapsed();

    println!("{}", "=".repeat(70));
    println!("🚀 RESULTS:");
    println!("{}", "=".repeat(70));
    println!("📊 Grammar Issues Fixed: {}", result.grammar_issues);
    println!("⭐ Style Score: {:.1}%", result.style_score);
    println!();
    println!("⏱️  TIMING BREAKDOWN:");
    println!("  STT (Whisper): {:.2}s", stt_time.as_secs_f64());
    println!("  Enhancement:   {:.2}s", enhance_time.as_secs_f64());
    println!("  ─────────────────────");
    println!("  Total:         {:.2}s", total_time.as_secs_f64());

    Ok(())
}

#[cfg(not(all(feature = "whisper", feature = "wisprflow")))]
fn main() {
    eprintln!("Error: This example requires both 'whisper' and 'wisprflow' features.");
    eprintln!("Run with: cargo run --example wisprflow_file_test --features whisper,wisprflow");
    std::process::exit(1);
}
