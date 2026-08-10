//! The published tag database (jugeeya.github.io/tags): read the manifest,
//! download tag zips, unpack the `.r2tag` inside — ready to feed into
//! `super::save::import_tags`. Install side only; sharing tags TO the site
//! stays in the Rivals 2 Tag Tool.

use std::io::Read;
use std::path::PathBuf;

const SITE_INDEX_URL: &str = "https://jugeeya.github.io/tags/data/index.json";
const SITE_DATA_BASE: &str = "https://jugeeya.github.io/tags/data";
const USER_AGENT: &str = "rivals-station-reporter";

fn str_at(v: &serde_json::Value, ptr: &str) -> String {
    v.pointer(ptr)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// A published tag from the site manifest.
#[derive(Debug, Clone)]
pub struct SharedTag {
    pub name: String,
    pub author: String,
    /// Manifest file name, e.g. `kim-e85def91.r2tag.zip`.
    pub file: String,
    /// start.gg user slug (`user/6192f6f1`) the tag is linked to, if any.
    pub startgg_slug: String,
    /// start.gg gamer tag, for display and collision renames.
    pub startgg_tag: String,
}

/// Read the site's published tag manifest (index.json).
pub async fn fetch_shared_tags() -> Result<Vec<SharedTag>, String> {
    let client = reqwest::Client::new();
    let res = client
        .get(SITE_INDEX_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!(
            "Could not load the tag database ({}).",
            res.status()
        ));
    }
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    if let Some(tags) = data.get("tags").and_then(|v| v.as_array()) {
        for t in tags {
            out.push(SharedTag {
                name: str_at(t, "/name"),
                author: str_at(t, "/author"),
                file: str_at(t, "/file"),
                startgg_slug: str_at(t, "/startgg/slug"),
                startgg_tag: str_at(t, "/startgg/tag"),
            });
        }
    }
    Ok(out)
}

fn extract_single_r2tag(zip_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_file() && entry.name().to_lowercase().ends_with(".r2tag") {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            return Ok(buf);
        }
    }
    Err("downloaded zip did not contain a .r2tag".into())
}

/// Download the named published tag zips into a scratch dir and unpack the
/// `.r2tag` inside each; returns the extracted paths in the same order as
/// `files` (manifest file names like `kim-e85def91.r2tag.zip`).
pub async fn download_tags(files: Vec<String>) -> Result<Vec<PathBuf>, String> {
    if files.is_empty() {
        return Err("No tags selected to install.".into());
    }

    let dir = std::env::temp_dir().join("rivals-station-reporter-tags");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let client = reqwest::Client::new();
    let mut written = Vec::new();
    for file in files {
        // Only fetch plain manifest file names — no path traversal.
        if file.contains('/')
            || file.contains('\\')
            || file.contains("..")
            || !file.ends_with(".r2tag.zip")
        {
            return Err(format!("Unexpected tag file name: {file}"));
        }

        let url = format!("{SITE_DATA_BASE}/{file}");
        let res = client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Download failed for {file} ({}).", res.status()));
        }
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        let r2tag = extract_single_r2tag(&bytes)?;

        let out_name = file.trim_end_matches(".zip"); // <stem>.r2tag
        let out_path = dir.join(out_name);
        std::fs::write(&out_path, &r2tag).map_err(|e| e.to_string())?;
        written.push(out_path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::extract_single_r2tag;
    use std::io::Write;

    fn zip_of(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            zw.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(payload).unwrap();
            zw.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn extracts_the_r2tag_from_a_zip() {
        let payload = b"GVAS fake save bytes for the test";
        let back = extract_single_r2tag(&zip_of("my-tag.r2tag", payload)).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn rejects_zip_without_r2tag() {
        assert!(extract_single_r2tag(&zip_of("readme.txt", b"nope")).is_err());
    }
}
