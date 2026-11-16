//! Compiles the vendored Rubber Band Library.
//!
//! Upstream ships `single/RubberBandSingle.cpp`, one translation unit that
//! `#include`s the rest of the sources and needs no external library: the
//! built-in FFT and resampler everywhere, Apple's vDSP on macOS, which is why
//! the Accelerate framework is linked there. Vendored rather than found on the
//! system so that a Windows runner, a macOS runner and this machine all build
//! the same code.

fn main() {
    println!("cargo:rerun-if-changed=vendor/rubberband");
    #[cfg(feature = "rubberband")]
    build();
}

/// Only compiled with the feature, because `cc` is only a dependency with it:
/// a build without Rubber Band must not need a C++ compiler at all.
#[cfg(feature = "rubberband")]
fn build() {
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .include("vendor/rubberband")
        .file("vendor/rubberband/single/RubberBandSingle.cpp")
        // Upstream's own warnings are upstream's business, and there are a
        // lot of them; ours are still on for our code.
        .warnings(false);

    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {
            // `windows.h` defines `min` and `max` as macros, and Rubber Band
            // calls `std::min`/`std::max` throughout: without this MSVC reports
            // a hundred `C2589: '(': illegal token on right side of '::'`.
            build.define("NOMINMAX", None);
            build.define("WIN32_LEAN_AND_MEAN", None);
        }
        Ok("macos") => {
            // The single-file build uses vDSP for its FFT on Apple platforms.
            println!("cargo:rustc-link-lib=framework=Accelerate");
        }
        _ => {}
    }

    build.compile("rubberband");
}
