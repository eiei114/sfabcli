//! Sketchfab's web API, called from inside a logged-in page.
//!
//! The browser holds the session; this module only shapes requests and reads responses.

use crate::cdp::{Page, head};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

/// All Sketchfab API paths sit under this prefix on the site origin.
const PREFIX: &str = "/i";

/// One response, kept raw so each command can decide how strict to be.
#[allow(dead_code)]
pub struct Reply {
    pub status: u16,
    pub json: Value,
    pub text: String,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A JSON field by slash path, or `Value::Null`.
    pub fn at(&self, path: &str) -> &Value {
        let mut node = &self.json;
        for key in path.split('/').filter(|k| !k.is_empty()) {
            node = match node.get(key) {
                Some(next) => next,
                None => return &Value::Null,
            };
        }
        node
    }

    pub fn str_at(&self, path: &str) -> Option<&str> {
        self.at(path).as_str()
    }

    pub fn array_at(&self, path: &str) -> &[Value] {
        self.at(path).as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    /// The first characters of the body, for an error message.
    pub fn brief(&self) -> String {
        let detail = self
            .str_at("detail")
            .map(str::to_owned)
            .unwrap_or_else(|| self.text.trim().to_owned());
        head(&detail, 200)
    }
}

/// Fetch one API path, adding the `/i` prefix when the caller did not supply it.
///
/// Archive URLs come back from the API already prefixed, so adding it unconditionally would
/// request `/i/i/...`.
pub fn get(page: &mut Page, path: &str) -> Result<Reply> {
    let full = if path.starts_with(PREFIX) {
        path.to_owned()
    } else {
        format!("{PREFIX}{path}")
    };
    let expression = format!(
        "(async()=>{{const r=await fetch({},{{credentials:'include',         headers:{{'accept':'application/json'}}}});const t=await r.text();         return JSON.stringify({{status:r.status,body:t}})}})()",
        json!(full)
    );
    let raw = page
        .evaluate_str(&expression)
        .with_context(|| format!("calling {full}"))?;
    let envelope: Value = serde_json::from_str(&raw).context("parsing the page envelope")?;
    let status = envelope["status"]
        .as_u64()
        .context("the page returned no status")? as u16;
    let text = envelope["body"].as_str().unwrap_or_default().to_owned();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(Reply { status, json, text })
}

/// A model's license, flattened for reporting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct License {
    pub slug: Option<String>,
    pub label: Option<String>,
    pub requirements: Option<String>,
    /// Free in the Creative Commons sense, not "costs nothing".
    pub free: bool,
    pub needs_attribution: bool,
    pub is_cc: bool,
}

/// Creative Commons slugs that allow a download without payment.
const FREE_SLUGS: &[&str] = &[
    "cc0", "by", "by-sa", "by-nd", "by-nc", "by-nc-sa", "by-nc-nd", "pd",
];

impl License {
    pub fn read(node: &Value) -> Self {
        let slug = node["slug"].as_str().map(str::to_owned);
        let free = node["free"].as_bool().unwrap_or_else(|| {
            slug.as_deref()
                .is_some_and(|s| FREE_SLUGS.contains(&s.to_lowercase().as_str()))
        });
        Self {
            needs_attribution: node["needsAttribution"].as_bool().unwrap_or(false),
            is_cc: node["isCc"].as_bool().unwrap_or(false),
            label: node["label"].as_str().map(str::to_owned),
            requirements: node["requirements"].as_str().map(str::to_owned),
            slug,
            free,
        }
    }
}

/// One texture-resolution variant of one archive format.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    /// Relative path that resolves to a signed S3 URL.
    pub url: String,
    pub size: Option<u64>,
    pub texture_max_resolution: Option<u64>,
}

/// Everything downloadable for one model.
#[derive(Debug, Default)]
pub struct ArchiveTypes {
    /// Format key (`glb`, `gltf`, `source`, `usdz`) to its variants.
    pub formats: Vec<(String, Vec<Variant>)>,
}

impl ArchiveTypes {
    pub fn variants(&self, format: &str) -> Option<&Vec<Variant>> {
        self.formats
            .iter()
            .find(|(key, _)| key == format)
            .map(|(_, variants)| variants)
    }

    pub fn available(&self) -> Vec<&str> {
        self.formats
            .iter()
            .filter(|(_, variants)| !variants.is_empty())
            .map(|(key, _)| key.as_str())
            .collect()
    }
}

/// Read the download variants for a model.
///
/// A 403 means the author disabled downloads or the model is paid.
pub fn archives(page: &mut Page, uid: &str) -> Result<ArchiveTypes> {
    let reply = get(page, &format!("/models/{uid}/download"))?;
    if !reply.ok() {
        bail!(
            "this model cannot be downloaded (HTTP {}){}",
            reply.status,
            match reply.str_at("detail") {
                Some(detail) => format!(": {detail}"),
                None => String::new(),
            }
        );
    }
    let mut types = ArchiveTypes::default();
    let Some(latest) = reply.at("latest").as_object() else {
        return Ok(types);
    };
    for (format, entries) in latest {
        let variants = entries
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|entry| {
                        Some(Variant {
                            url: entry["url"].as_str()?.to_owned(),
                            size: entry["size"].as_u64(),
                            texture_max_resolution: entry["textureMaxResolution"].as_u64(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        types.formats.push((format.clone(), variants));
    }
    Ok(types)
}

/// Resolve a relative archive path into its signed download URL.
pub fn resolve(page: &mut Page, path: &str) -> Result<String> {
    if path.starts_with("https://") {
        return Ok(path.to_owned());
    }
    let reply = get(page, path)?;
    if !reply.ok() {
        bail!("could not resolve the download URL: HTTP {}", reply.status);
    }
    reply
        .str_at("url")
        .map(str::to_owned)
        .with_context(|| format!("no signed url in the reply: {}", reply.brief()))
}

/// Pick the variant to download.
///
/// A requested texture resolution wins when it exists. Otherwise the smallest variant is chosen:
/// a 4K texture set is rarely wanted for a game import.
pub fn choose(variants: &[Variant], texture: Option<u64>) -> Option<&Variant> {
    if let Some(wanted) = texture
        && let Some(found) = variants
            .iter()
            .find(|v| v.texture_max_resolution == Some(wanted))
    {
        return Some(found);
    }
    variants.iter().min_by_key(|v| v.size.unwrap_or(u64::MAX))
}

/// A model's licence and download state, as one report.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub uid: String,
    pub name: String,
    pub by: Option<String>,
    pub author_username: Option<String>,
    pub author_url: Option<String>,
    pub license: License,
    pub download_type: Option<String>,
    pub downloadable: bool,
    pub faces: Option<u64>,
    pub vertices: Option<u64>,
    pub animations: Option<u64>,
    pub textures: Option<u64>,
    pub ai_generated: Option<bool>,
    pub price: Option<Value>,
    pub url: String,
    pub thumb: Option<String>,
}

impl Model {
    pub fn read(uid: &str, reply: &Reply) -> Self {
        let download_type = reply.str_at("downloadType").map(str::to_owned);
        Self {
            uid: uid.to_owned(),
            name: reply.str_at("name").unwrap_or(uid).to_owned(),
            by: reply
                .str_at("user/displayName")
                .or_else(|| reply.str_at("user/username"))
                .map(str::to_owned),
            author_username: reply.str_at("user/username").map(str::to_owned),
            author_url: reply.str_at("user/profileUrl").map(|url| {
                if url.starts_with("http") {
                    url.to_owned()
                } else {
                    format!("https://sketchfab.com{url}")
                }
            }),
            license: License::read(reply.at("license")),
            downloadable: download_type
                .as_deref()
                .is_some_and(|kind| kind != "notDownloadable"),
            download_type,
            faces: reply.at("faceCount").as_u64(),
            vertices: reply.at("vertexCount").as_u64(),
            animations: reply.at("animationCount").as_u64(),
            textures: reply.at("textureCount").as_u64(),
            ai_generated: reply.at("isAiGenerated").as_bool(),
            price: Some(reply.at("price").clone()).filter(|v| !v.is_null()),
            url: format!("https://sketchfab.com/3d-models/{uid}"),
            thumb: reply
                .at("thumbnails/images")
                .as_array()
                .and_then(|images| images.first())
                .and_then(|image| image["url"].as_str())
                .map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(json: Value) -> Reply {
        Reply {
            status: 200,
            json,
            text: String::new(),
        }
    }

    #[test]
    fn license_free_follows_the_slug_when_the_flag_is_absent() {
        let license = License::read(&json!({ "slug": "by", "label": "CC Attribution" }));
        assert!(license.free);
        assert!(!license.is_cc, "the API flag is missing, so it stays false");
    }

    #[test]
    fn non_cc_licenses_are_not_free() {
        let license = License::read(&json!({ "slug": "st", "free": false, "isCc": false }));
        assert!(!license.free);
    }

    #[test]
    fn smallest_variant_is_the_default() {
        let variants = vec![
            Variant {
                url: "a".into(),
                size: Some(900),
                texture_max_resolution: Some(4096),
            },
            Variant {
                url: "b".into(),
                size: Some(100),
                texture_max_resolution: Some(1024),
            },
            Variant {
                url: "c".into(),
                size: Some(500),
                texture_max_resolution: Some(2048),
            },
        ];
        assert_eq!(choose(&variants, None).map(|v| v.url.as_str()), Some("b"));
    }

    #[test]
    fn requested_texture_resolution_wins() {
        let variants = vec![
            Variant {
                url: "a".into(),
                size: Some(100),
                texture_max_resolution: Some(1024),
            },
            Variant {
                url: "b".into(),
                size: Some(900),
                texture_max_resolution: Some(4096),
            },
        ];
        assert_eq!(
            choose(&variants, Some(4096)).map(|v| v.url.as_str()),
            Some("b")
        );
    }

    #[test]
    fn unknown_texture_resolution_falls_back_to_smallest() {
        let variants = vec![
            Variant {
                url: "a".into(),
                size: Some(100),
                texture_max_resolution: Some(1024),
            },
            Variant {
                url: "b".into(),
                size: Some(900),
                texture_max_resolution: Some(4096),
            },
        ];
        assert_eq!(
            choose(&variants, Some(8192)).map(|v| v.url.as_str()),
            Some("a")
        );
    }

    #[test]
    fn model_is_not_downloadable_when_the_author_disabled_it() {
        let model = Model::read(
            "abc",
            &reply(json!({ "name": "X", "downloadType": "notDownloadable" })),
        );
        assert!(!model.downloadable);
    }

    #[test]
    fn model_without_a_download_type_is_not_downloadable() {
        let model = Model::read("abc", &reply(json!({ "name": "X" })));
        assert!(!model.downloadable);
    }
}
