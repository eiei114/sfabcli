//! Fetching model archives and recording where they came from.

use crate::cdp::head;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Anything larger than this would be a surprise for a single model archive.
const MAX_ARCHIVE: u64 = 8 << 30;

/// Stream a URL to disk, refusing to keep a partial file as if it were complete.
pub fn fetch(url: &str, dest: &Path, expected: Option<u64>) -> Result<u64> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .http_status_as_error(true)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("downloading {}", head(url, 120)))?;
    let source = response.body_mut().as_reader();
    let mut sink = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    let written = io::copy(&mut source.take(MAX_ARCHIVE), &mut sink)
        .with_context(|| format!("writing {}", dest.display()))?;
    sink.flush().context("flushing the download")?;
    drop(sink);

    if let Some(expected) = expected
        && written != expected
    {
        let _ = fs::remove_file(dest);
        bail!(
            "size mismatch for {}: expected {expected} bytes, got {written}",
            dest.display()
        );
    }
    Ok(written)
}

/// Extract an archive and return the directory it was written to.
pub fn unzip(archive: &Path, dir: &Path) -> Result<PathBuf> {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "extracted".to_owned());
    let target = dir.join(stem);
    let file = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(file).with_context(|| format!("reading {}", archive.display()))?;
    // The extractor sanitises entry names: an entry cannot escape the destination.
    zip.extract(&target)
        .with_context(|| format!("extracting {}", archive.display()))?;
    println!("unzipped -> {}", target.display());
    Ok(target)
}

/// Turn a model name into something safe to use as a file name.
pub fn safe_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// The provenance record written next to every download.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub uid: String,
    pub title: String,
    pub by: Option<String>,
    pub author_username: Option<String>,
    pub author_url: Option<String>,
    pub license: Option<String>,
    pub license_label: Option<String>,
    pub license_requirements: Option<String>,
    pub attribution_required: bool,
    pub ai_generated: Option<bool>,
    pub format: String,
    pub texture_max_resolution: Option<u64>,
    pub size_bytes: u64,
    pub file: String,
    pub source: String,
    pub fetched: String,
}

impl Source {
    fn write(&self, dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let short: String = self.uid.chars().take(8).collect();
        let path = dir.join(format!("SFAB_SOURCE_{short}.json"));
        let body = serde_json::to_string_pretty(self).context("serialising the source record")?;
        fs::write(&path, body + "\n").with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

/// RFC 3339 UTC, second precision.
pub fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let (year, month, day) = civil(secs / 86_400);
    let remaining = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remaining / 3600,
        (remaining % 3600) / 60,
        remaining % 60
    )
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil(days: u64) -> (u64, u64, u64) {
    let days = days as i64;
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as u64, month as u64, day as u64)
}

/// Write the provenance record for a finished download.
pub fn record(
    dir: &Path,
    model: &crate::api::Model,
    format: &str,
    texture: Option<u64>,
    size: u64,
    file: &str,
) -> Result<PathBuf> {
    let source = Source {
        uid: model.uid.clone(),
        title: model.name.clone(),
        by: model.by.clone(),
        author_username: model.author_username.clone(),
        author_url: model.author_url.clone(),
        license: model.license.slug.clone(),
        license_label: model.license.label.clone(),
        license_requirements: model.license.requirements.clone(),
        attribution_required: model.license.needs_attribution,
        ai_generated: model.ai_generated,
        format: format.to_owned(),
        texture_max_resolution: texture,
        size_bytes: size,
        file: file.to_owned(),
        source: model.url.clone(),
        fetched: now(),
    };
    source.write(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(civil(0), (1970, 1, 1));
    }

    #[test]
    fn a_known_date_is_civil() {
        // 2033-05-18T03:33:20Z
        assert_eq!(civil(2_000_000_000 / 86_400), (2033, 5, 18));
    }

    #[test]
    fn unsafe_characters_are_replaced() {
        assert_eq!(safe_name("Wooden/Barrel", "fallback"), "Wooden_Barrel");
        assert_eq!(safe_name("a:b*c?", "fallback"), "a_b_c_");
    }

    #[test]
    fn empty_name_falls_back() {
        assert_eq!(safe_name("", "abc123"), "abc123");
        assert_eq!(safe_name("   ", "abc123"), "abc123");
    }
}
