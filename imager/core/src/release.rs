//! Where images come from: the Raspberry Pi Imager repository JSON that every release
//! publishes (`pixelplus-imager.json`, same format Raspberry Pi Imager reads), found via
//! the GitHub releases API. Pure parsing; the GUI does the HTTP.

use serde::{Deserialize, Serialize};

pub const GITHUB_RELEASES: &str =
    "https://api.github.com/repos/tlchandler/PixelPlus/releases?per_page=10";
pub const REPO_ASSET: &str = "pixelplus-imager.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OsImage {
    pub name: String,
    pub description: String,
    pub url: String,
    pub release_date: String,
    pub version: String,
    pub prerelease: bool,
    pub download_size: Option<u64>,
    pub download_sha256: Option<String>,
    pub extract_size: Option<u64>,
    pub extract_sha256: Option<String>,
    pub recommended: bool,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// A release that the GUI should look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRef {
    pub version: String,
    pub prerelease: bool,
    pub published: String,
    /// URL of `pixelplus-imager.json`, if the release has one.
    pub repo_json_url: Option<String>,
    /// Fallback: bare image assets (no checksums known).
    pub images: Vec<OsImage>,
}

pub fn parse_github_releases(json: &str) -> Result<Vec<ReleaseRef>, serde_json::Error> {
    let rels: Vec<GhRelease> = serde_json::from_str(json)?;
    Ok(rels
        .into_iter()
        .filter(|r| !r.draft)
        .map(|r| {
            let version = r.tag_name.trim_start_matches('v').to_string();
            let published = r.published_at.clone().unwrap_or_default();
            let repo_json_url = r
                .assets
                .iter()
                .find(|a| a.name == REPO_ASSET)
                .map(|a| a.browser_download_url.clone());
            let images = r
                .assets
                .iter()
                .filter(|a| a.name.starts_with("pixelplus-") && a.name.ends_with("-arm64.img.xz"))
                .map(|a| OsImage {
                    name: format!(
                        "PixelPlus {version} ({})",
                        if a.name.contains("bookworm") {
                            "Bookworm"
                        } else {
                            "Trixie"
                        }
                    ),
                    description: r.name.clone().unwrap_or_default(),
                    url: a.browser_download_url.clone(),
                    release_date: published.get(..10).unwrap_or("").to_string(),
                    version: version.clone(),
                    prerelease: r.prerelease,
                    download_size: Some(a.size),
                    download_sha256: None,
                    extract_size: None,
                    extract_sha256: None,
                    recommended: !a.name.contains("bookworm"),
                })
                .collect();
            ReleaseRef {
                version,
                prerelease: r.prerelease,
                published,
                repo_json_url,
                images,
            }
        })
        .collect())
}

#[derive(Deserialize)]
struct RepoJson {
    os_list: Vec<RepoEntry>,
}

#[derive(Deserialize)]
struct RepoEntry {
    name: String,
    #[serde(default)]
    description: String,
    url: String,
    #[serde(default)]
    release_date: String,
    #[serde(default)]
    image_download_size: Option<u64>,
    #[serde(default)]
    image_download_sha256: Option<String>,
    #[serde(default)]
    extract_size: Option<u64>,
    #[serde(default)]
    extract_sha256: Option<String>,
    #[serde(default)]
    init_format: Option<String>,
}

/// Parse `pixelplus-imager.json` (Raspberry Pi Imager format) into images.
pub fn parse_repo_json(
    json: &str,
    version: &str,
    prerelease: bool,
) -> Result<Vec<OsImage>, serde_json::Error> {
    let r: RepoJson = serde_json::from_str(json)?;
    Ok(r.os_list
        .into_iter()
        .map(|e| OsImage {
            recommended: e.init_format.as_deref() == Some("cloudinit-rpi"),
            name: e.name,
            description: e.description,
            url: e.url,
            release_date: e.release_date,
            version: version.to_string(),
            prerelease,
            download_size: e.image_download_size,
            download_sha256: e.image_download_sha256,
            extract_size: e.extract_size,
            extract_sha256: e.extract_sha256,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_and_repo_json() {
        let gh = r#"[
          {"tag_name":"v1.1.0-beta.1","name":"Beta","draft":false,"prerelease":true,"published_at":"2026-11-02T10:00:00Z","assets":[]},
          {"tag_name":"v1.0.0","name":"PixelPlus 1.0","draft":false,"prerelease":false,"published_at":"2026-10-20T10:00:00Z","assets":[
            {"name":"pixelplus-1.0.0-trixie-arm64.img.xz","browser_download_url":"https://x/t.img.xz","size":900000000},
            {"name":"pixelplus-1.0.0-bookworm-arm64.img.xz","browser_download_url":"https://x/b.img.xz","size":800000000},
            {"name":"pixelplus-imager.json","browser_download_url":"https://x/pixelplus-imager.json","size":2000},
            {"name":"pixelplus_1.0.0_arm64.deb","browser_download_url":"https://x/p.deb","size":1}]},
          {"tag_name":"v0.9.0","draft":true,"prerelease":false,"assets":[]}
        ]"#;
        let r = parse_github_releases(gh).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].prerelease);
        assert_eq!(r[1].version, "1.0.0");
        assert_eq!(
            r[1].repo_json_url.as_deref(),
            Some("https://x/pixelplus-imager.json")
        );
        assert_eq!(r[1].images.len(), 2);
        assert!(r[1].images[0].recommended);
        assert_eq!(r[1].images[0].release_date, "2026-10-20");

        let repo = r#"{"imager":{"devices":[]},"os_list":[
          {"name":"PixelPlus 1.0.0 (Trixie, 64-bit)","description":"d","url":"https://x/t.img.xz","release_date":"2026-10-20",
           "extract_size":3000000000,"extract_sha256":"ab","image_download_size":900000000,"image_download_sha256":"cd","init_format":"cloudinit-rpi"}]}"#;
        let imgs = parse_repo_json(repo, "1.0.0", false).unwrap();
        assert_eq!(imgs[0].extract_sha256.as_deref(), Some("ab"));
        assert!(imgs[0].recommended);
    }
}
