//! sfab — operate Sketchfab from a terminal.
//!
//! Borrows the session from an already logged-in Chrome over the DevTools protocol and calls
//! Sketchfab's own web API from inside the page, so no OAuth token or cookie is stored here.

mod api;
mod cdp;
mod download;

use anyhow::{Context, Result, bail};
use api::{ArchiveTypes, License, Model};
use cdp::{Browser, Page};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;

const SKETCHFAB_ORIGIN: &str = "https://sketchfab.com";

#[derive(clap::Parser)]
#[command(
    name = "sfab",
    version,
    about = "Operate Sketchfab using a logged-in Chrome session",
    long_about = "Borrows a logged-in Chrome over CDP (default http://127.0.0.1:9223, override with \
                  SFAB_CDP) and calls Sketchfab's web API from inside the page.\n\n\
                  Downloadable formats are glTF, GLB, USDZ and source archives; Sketchfab does not \
                  publish standalone FBX or OBJ."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Open the Sketchfab login page in the CDP browser
    Login,
    /// Confirm the session and account type
    Status,
    /// Search models
    Search {
        /// Words to search for
        words: Vec<String>,
        /// Only models whose author allows download
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        downloadable: bool,
        /// Keep only these license slugs, comma separated: by, by-sa, by-nc, cc0
        #[arg(long)]
        license: Option<String>,
        /// Only animated models
        #[arg(long)]
        animated: bool,
        /// Only staff-picked models
        #[arg(long)]
        staffpicked: bool,
        /// Only models under this face count
        #[arg(long)]
        max_face: Option<u64>,
        /// Sort order
        #[arg(long, value_parser = ["relevance", "likes", "recent"], default_value = "relevance")]
        sort: String,
        #[arg(long, default_value_t = 24)]
        limit: usize,
        /// Fetch each model's license (slower: one request per result, required by --license)
        #[arg(long)]
        details: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show a model's license, download state and geometry
    Show {
        /// Model UID or URL
        uid: String,
    },
    /// Print what `download` would fetch, without downloading
    Plan {
        /// Model UID or URL
        uid: String,
        /// Archive format
        #[arg(long, default_value = "glb", value_parser = ["glb", "gltf", "source", "usdz"])]
        format: String,
        /// Texture resolution to prefer, e.g. 1024, 2048, 4096
        #[arg(long)]
        tex: Option<u64>,
    },
    /// List every format and texture variant with sizes
    ArchiveTypes {
        /// Model UID or URL
        uid: String,
    },
    /// Download a model archive
    Download {
        /// Model UID or URL
        uid: String,
        #[arg(long, default_value = "glb", value_parser = ["glb", "gltf", "source", "usdz"])]
        format: String,
        /// Texture resolution to prefer, e.g. 1024, 2048, 4096
        #[arg(long)]
        tex: Option<u64>,
        /// Destination directory
        #[arg(long, default_value = ".")]
        out: PathBuf,
        /// Extract the archive after downloading
        #[arg(long)]
        unzip: bool,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sfab: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let endpoint = std::env::var("SFAB_CDP").unwrap_or_else(|_| cdp::DEFAULT_ENDPOINT.to_owned());
    let browser = Browser::new(&endpoint);
    let cli = <Cli as clap::Parser>::parse();

    if let Command::Login = cli.command {
        browser.open(&format!("{SKETCHFAB_ORIGIN}/login"))?;
        println!("opened the login page; run `sfab status` when done");
        return Ok(());
    }

    let mut page = open_page(&browser)?;
    match cli.command {
        Command::Login => unreachable!(),
        Command::Status => status(&mut page),
        Command::Search {
            words,
            downloadable,
            license,
            animated,
            staffpicked,
            max_face,
            sort,
            limit,
            details,
            json,
        } => search(
            &mut page,
            &SearchRequest {
                query: words.join(" "),
                downloadable,
                license,
                animated,
                staffpicked,
                max_face,
                sort,
                limit,
                details,
            },
            json,
        ),
        Command::Show { uid } => show(&mut page, &uid),
        Command::Plan { uid, format, tex } => plan(&mut page, &uid, &format, tex),
        Command::ArchiveTypes { uid } => archive_types(&mut page, &uid),
        Command::Download {
            uid,
            format,
            tex,
            out,
            unzip,
        } => download(&mut page, &uid, &format, tex, &out, unzip),
    }
}

/// Reuse the Sketchfab tab, or open one and give it a moment to load.
fn open_page(browser: &Browser) -> Result<Page> {
    if let Some(target) = browser.find_page(SKETCHFAB_ORIGIN)? {
        return Page::connect(&target);
    }
    let target = browser
        .open(&format!("{SKETCHFAB_ORIGIN}/feed"))
        .context("opening a Sketchfab tab")?;
    std::thread::sleep(std::time::Duration::from_secs(4));
    Page::connect(&target)
}

fn status(page: &mut Page) -> Result<()> {
    let reply = api::get(page, "/users/me")?;
    if !reply.ok() {
        println!("not signed in (HTTP {}). Run `sfab login`", reply.status);
        std::process::exit(1);
    }
    let name = reply
        .str_at("displayName")
        .or_else(|| reply.str_at("username"))
        .unwrap_or("(no display name)");
    let account = reply.str_at("account").unwrap_or("unknown");
    println!("ok: {name} ({account}, signed in)");
    Ok(())
}

struct SearchRequest {
    query: String,
    downloadable: bool,
    license: Option<String>,
    animated: bool,
    staffpicked: bool,
    max_face: Option<u64>,
    sort: String,
    limit: usize,
    details: bool,
}

#[derive(Serialize)]
struct SearchRow {
    uid: String,
    name: String,
    by: Option<String>,
    license: Option<String>,
    license_requirements: Option<String>,
    download_type: Option<String>,
    download_count: Option<u64>,
    likes: Option<u64>,
    faces: Option<u64>,
    animated: bool,
    ai_generated: Option<bool>,
    price: Option<f64>,
    url: String,
}

fn search(page: &mut Page, request: &SearchRequest, json: bool) -> Result<()> {
    let mut path = format!(
        "/search?type=models&count={}&q={}",
        request.limit.clamp(1, 24),
        encode(&request.query)
    );
    if request.downloadable {
        path.push_str("&downloadable=true");
    }
    match request.sort.as_str() {
        "likes" => path.push_str("&sort_by=-likeCount"),
        "recent" => path.push_str("&sort_by=-publishedAt"),
        _ => path.push_str("&sort_by=-relevance"),
    }
    if request.animated {
        path.push_str("&animated=true");
    }
    if request.staffpicked {
        path.push_str("&staffpicked=true");
    }
    if let Some(faces) = request.max_face {
        path.push_str(&format!("&max_face_count={faces}"));
    }
    // The licence parameter on /search does nothing, so filtering happens after the fetch.

    let reply = api::get(page, &path)?;
    if !reply.ok() {
        bail!("search failed: HTTP {}: {}", reply.status, reply.brief());
    }

    let mut rows = Vec::new();
    for model in reply.array_at("results") {
        let uid = model["uid"].as_str().unwrap_or_default();
        // Results carry no licence, so a filtering request needs one detail call per model.
        // That reply also carries the face count, so nothing is fetched twice.
        let detail = if request.details {
            let reply = api::get(page, &format!("/models/{uid}"))?;
            Some(reply).filter(|r| r.ok())
        } else {
            None
        };
        let license = detail
            .as_ref()
            .map(|reply| License::read(reply.at("license")));
        let faces = detail
            .as_ref()
            .and_then(|reply| reply.at("faceCount").as_u64())
            .or_else(|| model["faceCount"].as_u64());
        rows.push(SearchRow {
            uid: uid.to_owned(),
            name: model["name"].as_str().unwrap_or_default().to_owned(),
            by: model
                .pointer("/user/displayName")
                .or_else(|| model.pointer("/user/username"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            license: license.as_ref().and_then(|l| l.slug.clone()),
            license_requirements: license.as_ref().and_then(|l| l.requirements.clone()),
            download_type: model["downloadType"].as_str().map(str::to_owned),
            download_count: model["downloadCount"].as_u64(),
            likes: model["likeCount"].as_u64(),
            faces,
            animated: model["animationCount"].as_u64().unwrap_or(0) > 0,
            ai_generated: model["isAiGenerated"].as_bool(),
            price: model["price"].as_f64(),
            url: model["viewerUrl"]
                .as_str()
                .map(|url| {
                    if url.starts_with("http") {
                        url.to_owned()
                    } else {
                        format!("{SKETCHFAB_ORIGIN}{url}")
                    }
                })
                .unwrap_or_else(|| format!("{SKETCHFAB_ORIGIN}/3d-models/{uid}")),
        });
    }

    if let Some(wanted) = &request.license {
        if !request.details {
            bail!("--license needs --details: search results carry no license data");
        }
        let wanted: Vec<&str> = wanted.split(',').map(str::trim).collect();
        rows.retain(|row| {
            row.license
                .as_deref()
                .is_some_and(|slug| wanted.contains(&slug))
        });
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for row in &rows {
        let tag = [
            row.license.clone().unwrap_or_else(|| "licence?".to_owned()),
            row.download_type
                .clone()
                .unwrap_or_else(|| "dl?".to_owned()),
            row.faces.map(|f| format!("{f}f")).unwrap_or_default(),
            if row.animated {
                "anim".to_owned()
            } else {
                String::new()
            },
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        println!(
            "{}  {}  {}  — {}",
            row.uid,
            tag,
            row.name,
            row.by.as_deref().unwrap_or("unknown")
        );
    }
    println!("\n{} results. See `sfab show <uid>` for one", rows.len());
    Ok(())
}

fn show(page: &mut Page, uid: &str) -> Result<()> {
    let uid = uid_of(uid);
    let reply = api::get(page, &format!("/models/{uid}"))?;
    if !reply.ok() {
        bail!("model {uid}: HTTP {}", reply.status);
    }
    let model = Model::read(&uid, &reply);
    let categories: Vec<&str> = reply
        .array_at("categories")
        .iter()
        .filter_map(|c| c["name"].as_str().or_else(|| c["slug"].as_str()))
        .collect();
    let tags: Vec<&str> = reply
        .array_at("tags")
        .iter()
        .take(10)
        .filter_map(|t| t["name"].as_str().or_else(|| t.as_str()))
        .collect();

    let report = json!({
        "uid": model.uid,
        "name": model.name,
        "by": model.by,
        "license": model.license,
        "downloadType": model.download_type,
        "isDownloadable": model.downloadable,
        "faces": model.faces,
        "vertices": model.vertices,
        "animations": model.animations,
        "textures": model.textures,
        "aiGenerated": model.ai_generated,
        "categories": categories,
        "tags": tags,
        "price": model.price,
        "url": model.url,
        "thumb": model.thumb,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Everything a download would do, resolved but not fetched.
fn plan(page: &mut Page, uid: &str, format: &str, tex: Option<u64>) -> Result<()> {
    let uid = uid_of(uid);
    let (model, variants) = resolve_plan(page, &uid, format, tex)?;
    let picked = api::choose(&variants, tex).context("no variants available")?;
    let report = json!({
        "uid": model.uid,
        "name": model.name,
        "by": model.by,
        "format": format,
        "textureMaxResolution": picked.texture_max_resolution,
        "sizeBytes": picked.size,
        "sizeMB": picked.size.map(|size| format!("{:.1}MB", size as f64 / 1e6)),
        "license": model.license,
        "attributionRequired": model.license.needs_attribution,
        "aiGenerated": model.ai_generated,
        "url": model.url,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn archive_types(page: &mut Page, uid: &str) -> Result<()> {
    let uid = uid_of(uid);
    let types = api::archives(page, &uid)?;
    if types.available().is_empty() {
        bail!("no downloadable archives");
    }
    let formats: Vec<Value> = types
        .formats
        .iter()
        .map(|(format, variants)| {
            json!({
                "format": format,
                "variants": variants
                    .iter()
                    .map(|v| json!({ "tex": v.texture_max_resolution, "size": v.size }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "uid": uid, "formats": formats }))?
    );
    Ok(())
}

fn download(
    page: &mut Page,
    uid: &str,
    format: &str,
    tex: Option<u64>,
    out: &PathBuf,
    unzip: bool,
) -> Result<()> {
    let uid = uid_of(uid);
    let (model, variants) = resolve_plan(page, &uid, format, tex)?;
    let picked = api::choose(&variants, tex).context("no variants available")?;
    let url = api::resolve(page, &picked.url)?;

    let dir = std::path::absolute(out).unwrap_or_else(|_| out.clone());
    let extension = match format {
        "gltf" | "source" => "zip",
        "usdz" => "usdz",
        _ => "glb",
    };
    let stem = download::safe_name(&model.name, &uid);
    let dest = dir.join(format!("{stem}.{extension}"));
    let written = download::fetch(&url, &dest, picked.size)?;
    println!("saved {} ({:.1}MB)", dest.display(), written as f64 / 1e6);

    if unzip && extension == "zip" {
        download::unzip(&dest, &dir)?;
    }

    let record = download::record(
        &dir,
        &model,
        format,
        picked.texture_max_resolution,
        written,
        &dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )?;
    println!("record {}", record.display());
    if model.license.needs_attribution {
        println!(
            "ATTRIBUTION REQUIRED: \"{}\" by {} ({}) — {}",
            model.name,
            model.by.as_deref().unwrap_or("unknown"),
            model.url,
            model.license.label.as_deref().unwrap_or("see the license")
        );
    }
    Ok(())
}

/// Load a model and its variants, refusing anything the author did not publish as downloadable.
fn resolve_plan(
    page: &mut Page,
    uid: &str,
    format: &str,
    _tex: Option<u64>,
) -> Result<(Model, Vec<api::Variant>)> {
    let detail = api::get(page, &format!("/models/{uid}"))?;
    if !detail.ok() {
        bail!("model {uid}: HTTP {}", detail.status);
    }
    let model = Model::read(uid, &detail);
    let types: ArchiveTypes = api::archives(page, uid)?;
    let variants = match types.variants(format) {
        Some(variants) if !variants.is_empty() => variants.clone(),
        _ => {
            let available = types.available().join(", ");
            bail!("format {format} is not offered. Available: {available}");
        }
    };
    if !model.downloadable {
        bail!("the author does not allow downloads for this model");
    }
    Ok((model, variants))
}

/// Sketchfab UIDs are exactly 32 hex characters.
const UID_LEN: usize = 32;

/// Pull the model UID out of a URL, or accept a bare UID.
///
/// Viewer URLs carry a slug in front of the UID (`.../barrel-ef2024...`), so the UID is the first
/// 32-character hex run that is not part of a longer one.
fn uid_of(input: &str) -> String {
    let trimmed = input.trim();
    let bytes = trimmed.as_bytes();
    for start in 0..bytes.len() {
        let end = start + UID_LEN;
        if end > bytes.len() {
            break;
        }
        let inside = bytes[start..end].iter().all(u8::is_ascii_hexdigit);
        let left_edge = start == 0 || !bytes[start - 1].is_ascii_hexdigit();
        let right_edge = end == bytes.len() || !bytes[end].is_ascii_hexdigit();
        if inside && left_edge && right_edge {
            return trimmed[start..end].to_owned();
        }
    }
    trimmed.to_owned()
}

/// Percent-encode a query value, keeping only the unreserved set.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_comes_out_of_a_url() {
        let url = "https://sketchfab.com/3d-models/barrel-ef2024160ce640558614d2d34d1e3c47";
        assert_eq!(uid_of(url), "ef2024160ce640558614d2d34d1e3c47");
    }

    #[test]
    fn bare_uid_is_unchanged() {
        let uid = "ef2024160ce640558614d2d34d1e3c47";
        assert_eq!(uid_of(uid), uid);
    }

    #[test]
    fn uid_comes_out_of_a_slugged_url() {
        // The real viewer URL form is slug-uid, not a bare uid.
        let url = "https://sketchfab.com/3d-models/wooden-barrel-1f088de099d24cea9d8bba2a3d067403";
        assert_eq!(uid_of(url), "1f088de099d24cea9d8bba2a3d067403");
    }

    #[test]
    fn uid_comes_out_of_a_models_url() {
        let url = "https://sketchfab.com/models/ef2024160ce640558614d2d34d1e3c47/embed";
        assert_eq!(uid_of(url), "ef2024160ce640558614d2d34d1e3c47");
    }

    #[test]
    fn non_uid_input_passes_through() {
        assert_eq!(uid_of("barrel"), "barrel");
    }

    #[test]
    fn the_exact_live_url_yields_its_uid() {
        let url = "https://sketchfab.com/3d-models/barrel-ef2024160ce640558614d2d34d1e3c47";
        assert_eq!(uid_of(url), "ef2024160ce640558614d2d34d1e3c47");
    }

    #[test]
    fn a_longer_hex_run_is_not_truncated_into_a_uid() {
        let digits = "a".repeat(40);
        assert_eq!(uid_of(&digits), digits);
    }

    #[test]
    fn query_is_encoded() {
        assert_eq!(encode("wooden barrel"), "wooden+barrel");
        assert_eq!(encode("a&b"), "a%26b");
    }
}
