//! CS Engine identity — the single source of every user-visible name of the engine.
//!
//! CS Engine (by Craft Studio) is built on Horizon Android (MIT); upstream code keeps its internal
//! names, and every place a player can read reads from here instead. Rebranding, versioning or
//! retitling the engine therefore never touches upstream files again: change the constants, rebuild.
//!
//! Versions: `CS_ENGINE_VERSION` (CI sets it from the release tag) is the product version; the
//! Horizon core version the build is based on is reported next to it in crash reports and F3, so a
//! report always says which upstream parity level it came from.

/// The engine's name as players read it.
pub const NAME: &str = "Masterix Engine";
/// Short form for tight spaces (F3 brand, file prefixes).
pub const SHORT: &str = "Masterix";
/// The maker.
pub const VENDOR: &str = "AbhayTheMaster";
/// The line under the name.
pub const TAGLINE: &str = "Powered by AbhayTheMaster";
/// The brand reported where vanilla reports "vanilla" (F3 `(<version>/<brand>)`, server brand).
pub const BRAND: &str = "masterix-engine";
/// The edition word drawn under the Minecraft logo in place of "JAVA" ("CS EDITION").
pub const EDITION: &str = "MASTERIX";
/// The native library (`lib<LIB>.so`, `android.app.lib_name`).
pub const LIB: &str = "cs_engine";
/// The name of the upstream project this engine is built on (credits, notices).
pub const UPSTREAM: &str = "Horizon Android";
/// Shown on the title screen as Mojang's guidelines ask of every third-party client.
pub const NOT_AFFILIATED: &str = "Not affiliated with Mojang or Microsoft";

/// Product version: `CS_ENGINE_VERSION` from the build (release tag), else this crate's version.
pub const VERSION: &str = match option_env!("CS_ENGINE_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Build commit (short) when CI provides it.
pub const COMMIT: Option<&str> = option_env!("CS_ENGINE_COMMIT");

/// `CS Engine 1.0.0`
pub fn name_version() -> String {
    format!("{NAME} {VERSION}")
}

/// `CS Engine 1.0.0 · Minecraft Java 26.3` — the title screen's version line.
pub fn title_line(minecraft_version: &str) -> String {
    format!("{NAME} {VERSION} · Minecraft Java {minecraft_version}")
}

/// `CS Engine 1.0.0 (core 0.7.7, abc1234)` — reports, logs, user agents.
pub fn full_version(core_version: &str) -> String {
    match COMMIT {
        Some(commit) if !commit.is_empty() => {
            let short = commit.get(..7).unwrap_or(commit);
            format!("{NAME} {VERSION} (core {core_version}, {short})")
        }
        _ => format!("{NAME} {VERSION} (core {core_version})"),
    }
}

/// `---- CS Engine Crash Report ----`
pub fn crash_report_title() -> String {
    format!("---- {NAME} Crash Report ----")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_read_as_designed() {
        assert_eq!(title_line("26.3"), format!("Masterix Engine {VERSION} · Minecraft Java 26.3"));
        assert!(full_version("0.7.7").starts_with("Masterix Engine "));
        assert!(full_version("0.7.7").contains("core 0.7.7"));
        assert_eq!(crash_report_title(), "---- Masterix Engine Crash Report ----");
        assert_eq!(LIB, "cs_engine");
        assert!(!NAME.contains("Horizon"));
    }
}

/// The CS Launcher icon (64x64 PNG) worlds hosted from CS Engine show in friends'
/// server lists when no `cs-server-icon.png` is in the game folder.
pub const SERVER_ICON: &[u8] = include_bytes!("../cs-server-icon.png");
