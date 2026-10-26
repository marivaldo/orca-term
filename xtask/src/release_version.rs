//! Release-tag lint: a release tag reads exactly `v<version>`, the version being the one in
//! `Cargo.toml`, so the assets the tag names carry the version the binary reports in `--version`,
//! the JSON `version` and `doctor`. A pre-release suffix (`v0.1.0-rc.1`) passes only when the
//! Cargo version carries the same one.

/// Returns every rule the tag breaks against the crate's `version`.
pub(crate) fn lint(tag: &str, version: &str) -> Vec<String> {
    let expected = format!("v{version}");
    if tag == expected {
        Vec::new()
    } else {
        vec![format!(
            "the release tag must be `{expected}`, the version in Cargo.toml; got `{tag}`"
        )]
    }
}

/// Reads the `version` of the `[package]` table in a `Cargo.toml`, or `None` when it has none.
pub(crate) fn package_version(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "version" {
            return value.trim().strip_prefix('"')?.strip_suffix('"');
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_tag_of_the_cargo_version() {
        assert!(lint("v0.1.0", "0.1.0").is_empty());
        assert!(lint("v0.1.0-rc.1", "0.1.0-rc.1").is_empty());
    }

    #[test]
    fn rejects_a_tag_without_v_or_of_another_version() {
        assert_eq!(lint("0.1.0", "0.1.0").len(), 1);
        assert_eq!(lint("v0.1.1", "0.1.0").len(), 1);
        assert_eq!(lint("v0.2.0", "0.1.0").len(), 1);
    }

    #[test]
    fn rejects_a_pre_release_the_cargo_version_does_not_carry() {
        assert_eq!(lint("v0.1.0-rc.1", "0.1.0").len(), 1);
        assert_eq!(lint("v0.1.0", "0.1.0-rc.1").len(), 1);
    }

    #[test]
    fn reads_the_package_version_only() {
        let manifest = "[package]\nname = \"orca-term\"\nversion = \"0.1.0\"\n\n\
                        [dependencies]\nanyhow = { version = \"1\" }\n";
        assert_eq!(package_version(manifest), Some("0.1.0"));
        let later = "[workspace]\nmembers = []\n\n[package]\nversion = \"0.2.0\"\n";
        assert_eq!(package_version(later), Some("0.2.0"));
        assert_eq!(package_version("[dependencies]\nversion = \"1\"\n"), None);
    }
}
