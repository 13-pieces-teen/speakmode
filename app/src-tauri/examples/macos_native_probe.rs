//! Explicit debug-only native acceptance lab; see docs/macos.md.
#[cfg(all(target_os="macos",debug_assertions))]
fn main() {
    if let Err(error)=speakmode_lib::platform::macos::lab::run(std::env::args().skip(1)) {
        eprintln!("native probe failed: {error}"); std::process::exit(1);
    }
}
#[cfg(not(all(target_os="macos",debug_assertions)))]
fn main() { eprintln!("This lab requires a macOS debug build."); std::process::exit(2); }
