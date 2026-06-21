fn main() {
    println!("cargo:rustc-env=GGML_LOG_DISABLE=1");
    println!("cargo:rustc-env=WHISPER_LOG_DISABLE=1");

    // GCC/Clang-style native tuning flags; not valid on MSVC link.exe
    if !cfg!(target_os = "windows") {
        println!("cargo:rustc-link-arg=-march=native");
        println!("cargo:rustc-link-arg=-mtune=native");
    }
}
