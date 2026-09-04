//! OS/architecture identification for GitHub release asset names.
//!
//! `std::env::consts::OS` reports `"macos"`, but goreleaser's `name_template`
//! (`.goreleaser.yaml`) names assets using Go's `GOOS` vocabulary, where
//! macOS is `"darwin"`. Passing `consts::OS` straight into a filename would
//! silently build the wrong name and every macOS self-update would 404
//! against a real asset list — so [`Os::current`] goes through an explicit
//! match table instead, and every case (including the `"macos"` one) is
//! table-tested below.

use std::fmt;

/// The archive format goreleaser packages release binaries in, chosen per
/// OS (`.goreleaser.yaml`'s `format_overrides`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// `.tar.gz` — Linux and macOS.
    TarGz,
    /// `.zip` — Windows. The Go reference implementation never handled
    /// this case at all (it only ever unpacked tar+gzip), so every Windows
    /// self-update silently failed; this crate implements both.
    Zip,
}

impl ArchiveFormat {
    /// The filename extension goreleaser's `name_template` appends for
    /// this format (`"tar.gz"` or `"zip"`, no leading dot).
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::Zip => "zip",
        }
    }
}

/// The three operating systems penguin ships releases for, named after
/// goreleaser's `.Os` template variable (Go's `GOOS` spelling) rather than
/// `std::env::consts::OS`'s own spelling — the two disagree on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    /// Maps the running process's OS to the release-asset vocabulary.
    /// `None` for any OS this workspace does not ship binaries for (see
    /// `client.md`'s platform matrix) — never guessed at.
    pub fn current() -> Option<Os> {
        os_from_str(std::env::consts::OS)
    }

    /// The token goreleaser's `{{ .Os }}` substitutes into the asset name
    /// (`"linux"`, `"darwin"`, `"windows"`) — deliberately NOT the same as
    /// this variant's Rust-side name for [`Os::Macos`].
    pub fn asset_token(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "darwin",
            Os::Windows => "windows",
        }
    }

    /// The archive format this OS's release assets are packaged in.
    pub fn archive_format(self) -> ArchiveFormat {
        match self {
            Os::Windows => ArchiveFormat::Zip,
            Os::Linux | Os::Macos => ArchiveFormat::TarGz,
        }
    }

    /// The suffix the running executable's filename carries — `".exe"` on
    /// Windows, empty everywhere else.
    pub fn exe_suffix(self) -> &'static str {
        match self {
            Os::Windows => ".exe",
            Os::Linux | Os::Macos => "",
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asset_token())
    }
}

/// The pure `str -> Os` mapping, split out from [`Os::current`] so every
/// branch (in particular the `"macos" -> Os::Macos` one this module exists
/// to get right) is directly unit-testable without depending on which OS
/// the test suite happens to run on.
fn os_from_str(value: &str) -> Option<Os> {
    match value {
        "linux" => Some(Os::Linux),
        "macos" => Some(Os::Macos),
        "windows" => Some(Os::Windows),
        _ => None,
    }
}

/// The two CPU architectures penguin ships releases for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Amd64,
    Arm64,
}

impl Arch {
    /// Maps the running process's architecture to the release-asset
    /// vocabulary. `None` for anything this workspace does not ship
    /// binaries for.
    pub fn current() -> Option<Arch> {
        arch_from_str(std::env::consts::ARCH)
    }

    /// The token goreleaser's `{{ .Arch }}` substitutes into the asset name
    /// (`"amd64"`, `"arm64"`) — Rust's own `x86_64`/`aarch64` spellings
    /// never appear in a release filename.
    pub fn asset_token(self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asset_token())
    }
}

/// The pure `str -> Arch` mapping backing [`Arch::current`]; see
/// [`os_from_str`]'s doc for why this is split out.
fn arch_from_str(value: &str) -> Option<Arch> {
    match value {
        "x86_64" => Some(Arch::Amd64),
        "aarch64" => Some(Arch::Arm64),
        _ => None,
    }
}

/// Strips a leading `v`/`V` from a version string. goreleaser's `.Version`
/// template variable (used in the asset name) never carries the tag's `v`
/// prefix even though `.Tag` does, so a GitHub release tag (`"v0.2.0"`) has
/// to go through this before it can be used to build an asset filename or
/// compared against the running binary's own (unprefixed) version.
pub fn normalize_version(version: &str) -> &str {
    version.strip_prefix(['v', 'V']).unwrap_or(version)
}

/// Builds the exact asset filename goreleaser produces for a release,
/// e.g. `penguin_0.2.0_linux_amd64.tar.gz` — matches `.goreleaser.yaml`'s
/// `name_template` (`penguin_{{ .Version }}_{{ .Os }}_{{ .Arch }}`) exactly.
/// `version` must already be normalized (see [`normalize_version`]).
pub fn asset_filename(version: &str, os: Os, arch: Arch) -> String {
    format!(
        "penguin_{version}_{os}_{arch}.{ext}",
        os = os.asset_token(),
        arch = arch.asset_token(),
        ext = os.archive_format().extension(),
    )
}

/// The expected filename of a given binary inside the downloaded archive
/// for `os` — e.g. `("penguind", Os::Windows)` -> `"penguind.exe"`.
pub fn binary_filename(binary_name: &str, os: Os) -> String {
    format!("{binary_name}{}", os.exe_suffix())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_from_str_maps_macos_to_darwin_token() {
        // The one mapping this module exists to get right: Rust's
        // `std::env::consts::OS` says "macos", goreleaser's asset names say
        // "darwin" — the two must never be conflated.
        assert_eq!(os_from_str("macos"), Some(Os::Macos));
        assert_eq!(Os::Macos.asset_token(), "darwin");
    }

    #[test]
    fn os_from_str_covers_every_shipped_os() {
        assert_eq!(os_from_str("linux"), Some(Os::Linux));
        assert_eq!(Os::Linux.asset_token(), "linux");
        assert_eq!(os_from_str("windows"), Some(Os::Windows));
        assert_eq!(Os::Windows.asset_token(), "windows");
    }

    #[test]
    fn os_from_str_rejects_unshipped_platforms() {
        assert_eq!(os_from_str("freebsd"), None);
        assert_eq!(os_from_str(""), None);
        assert_eq!(os_from_str("Linux"), None);
    }

    #[test]
    fn arch_from_str_maps_rust_triples_to_asset_tokens() {
        assert_eq!(arch_from_str("x86_64"), Some(Arch::Amd64));
        assert_eq!(Arch::Amd64.asset_token(), "amd64");
        assert_eq!(arch_from_str("aarch64"), Some(Arch::Arm64));
        assert_eq!(Arch::Arm64.asset_token(), "arm64");
    }

    #[test]
    fn arch_from_str_rejects_unshipped_architectures() {
        assert_eq!(arch_from_str("arm"), None);
        assert_eq!(arch_from_str("x86"), None);
        assert_eq!(arch_from_str(""), None);
    }

    #[test]
    fn archive_format_is_zip_only_on_windows() {
        assert_eq!(Os::Windows.archive_format(), ArchiveFormat::Zip);
        assert_eq!(Os::Linux.archive_format(), ArchiveFormat::TarGz);
        assert_eq!(Os::Macos.archive_format(), ArchiveFormat::TarGz);
    }

    #[test]
    fn exe_suffix_is_dot_exe_only_on_windows() {
        assert_eq!(Os::Windows.exe_suffix(), ".exe");
        assert_eq!(Os::Linux.exe_suffix(), "");
        assert_eq!(Os::Macos.exe_suffix(), "");
    }

    #[test]
    fn normalize_version_strips_leading_v() {
        assert_eq!(normalize_version("v0.2.0"), "0.2.0");
        assert_eq!(normalize_version("V0.2.0"), "0.2.0");
        assert_eq!(normalize_version("0.2.0"), "0.2.0");
        assert_eq!(normalize_version(""), "");
    }

    #[test]
    fn asset_filename_matches_goreleaser_name_template_linux_amd64() {
        assert_eq!(
            asset_filename("0.2.0", Os::Linux, Arch::Amd64),
            "penguin_0.2.0_linux_amd64.tar.gz"
        );
    }

    #[test]
    fn asset_filename_matches_goreleaser_name_template_macos_arm64() {
        // The critical case: macOS in the filename must read "darwin", not
        // "macos", and the extension must still be tar.gz (not zip).
        assert_eq!(
            asset_filename("0.2.0", Os::Macos, Arch::Arm64),
            "penguin_0.2.0_darwin_arm64.tar.gz"
        );
    }

    #[test]
    fn asset_filename_matches_goreleaser_name_template_windows_amd64_uses_zip() {
        assert_eq!(
            asset_filename("0.2.0", Os::Windows, Arch::Amd64),
            "penguin_0.2.0_windows_amd64.zip"
        );
    }

    #[test]
    fn binary_filename_appends_exe_suffix_only_on_windows() {
        assert_eq!(binary_filename("penguind", Os::Windows), "penguind.exe");
        assert_eq!(binary_filename("penguind", Os::Linux), "penguind");
        assert_eq!(binary_filename("penguind", Os::Macos), "penguind");
    }
}
