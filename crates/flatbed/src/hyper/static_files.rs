//! Static-file serving for `static_route!` mounts.
//!
//! Files are read from the container filesystem at request time. Path
//! resolution rejects any `..` or absolute component in the request URL;
//! symlinks under `dir` are followed by the OS and are not separately
//! constrained (the served directory is operator-controlled, shipped in the
//! image).

use std::path::{Component, Path, PathBuf};

use crate::{HeaderName, HeaderValue, ResponseParts, StaticRouteInfo};

/// Serve a static file for `path` from the first mount that produces a hit.
///
/// Returns `None` when no mount matches, the resolved file is absent, and no
/// fallback applies.
///
/// The full file is read regardless of request method; the caller strips the
/// body for a HEAD request while preserving the `Content-Length` it implies.
pub async fn serve(routes: &[StaticRouteInfo], path: &str) -> Option<ResponseParts> {
    for route in routes {
        if let Some(parts) = serve_one(route, path).await {
            return Some(parts);
        }
    }
    None
}

async fn serve_one(route: &StaticRouteInfo, path: &str) -> Option<ResponseParts> {
    let rel = strip_mount(route.mount, path)?;
    let relative = sanitize(rel)?;
    let dir = Path::new(route.dir);

    if !relative.as_os_str().is_empty() {
        let full = dir.join(&relative);
        if let Some(bytes) = read_file(&full).await {
            return Some(build(bytes, &full));
        }
        if is_known_asset(&relative) {
            return None;
        }
    }

    let fallback = route.fallback?;
    let full = dir.join(fallback);
    let bytes = read_file(&full).await?;
    Some(build(bytes, &full))
}

/// Strip the mount prefix from a request path, returning the relative remainder.
///
/// The prefix must end on a path boundary, so mount `/assets` matches
/// `/assets/app.js` but not `/assetsX`.
fn strip_mount<'a>(mount: &str, path: &'a str) -> Option<&'a str> {
    let mount = mount.trim_end_matches('/');
    if mount.is_empty() {
        return Some(path.trim_start_matches('/'));
    }
    if path == mount {
        return Some("");
    }
    path.strip_prefix(mount)?.strip_prefix('/')
}

/// Resolve a relative request path to a directory-relative path, rejecting any
/// URL component that could escape the mount directory (`..`, absolute roots).
fn sanitize(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

async fn read_file(path: &Path) -> Option<Vec<u8>> {
    // `read` errors on a missing path and on a directory, so it doubles as the
    // existence/regular-file check — no separate `stat` needed.
    tokio::fs::read(path).await.ok()
}

fn build(bytes: Vec<u8>, path: &Path) -> ResponseParts {
    let mut parts = ResponseParts::ok(bytes, content_type(path));
    parts.headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static(cache_control(path)),
    );
    parts
}

/// Files a bundler content-hashes into their filename are safe to cache
/// forever; files that keep a stable name across builds (HTML, and the
/// conventionally-unhashed `robots.txt` / `favicon.ico` / `manifest.json` /
/// sitemap set) must stay fresh.
fn cache_control(path: &Path) -> &'static str {
    match extension(path).as_deref() {
        Some("html" | "json" | "txt" | "ico" | "xml" | "webmanifest") | None => "no-cache",
        _ => "public, max-age=31536000, immutable",
    }
}

/// Whether the path names a recognized asset type (its extension maps to a
/// concrete media type).
fn is_known_asset(path: &Path) -> bool {
    content_type(path) != "application/octet-stream"
}

fn content_type(path: &Path) -> &'static str {
    match extension(path).as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("xml") => "application/xml",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_mount_root() {
        assert_eq!(strip_mount("/", "/index.html"), Some("index.html"));
        assert_eq!(strip_mount("/", "/"), Some(""));
        assert_eq!(strip_mount("/", "/assets/app.js"), Some("assets/app.js"));
    }

    #[test]
    fn strip_mount_prefix_needs_boundary() {
        assert_eq!(strip_mount("/assets", "/assets/app.js"), Some("app.js"));
        assert_eq!(strip_mount("/assets", "/assets"), Some(""));
        assert_eq!(strip_mount("/assets", "/assetsX"), None);
        assert_eq!(strip_mount("/assets", "/other"), None);
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize("../etc/passwd").is_none());
        assert!(sanitize("a/../../b").is_none());
        assert!(sanitize("/abs").is_none());
    }

    #[test]
    fn sanitize_allows_nested() {
        assert_eq!(
            sanitize("assets/app.js"),
            Some(PathBuf::from("assets/app.js"))
        );
        assert_eq!(sanitize("./app.js"), Some(PathBuf::from("app.js")));
        assert_eq!(sanitize(""), Some(PathBuf::new()));
    }

    #[test]
    fn content_type_by_extension() {
        assert_eq!(
            content_type(Path::new("i.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("a.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("a.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new("f.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("x.bin")), "application/octet-stream");
    }

    #[test]
    fn cache_control_hashed_vs_stable_name() {
        // Content-hashed assets: cache forever.
        assert_eq!(
            cache_control(Path::new("app-a1b2.js")),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control(Path::new("style-c3d4.css")),
            "public, max-age=31536000, immutable"
        );
        // Stable-name files: must stay fresh.
        for stable in [
            "index.html",
            "robots.txt",
            "favicon.ico",
            "manifest.json",
            "sitemap.xml",
            "app.webmanifest",
        ] {
            assert_eq!(cache_control(Path::new(stable)), "no-cache", "{stable}");
        }
    }

    #[test]
    fn known_asset_vs_client_route() {
        for asset in [
            "app.js",
            "index.html",
            "sitemap.xml",
            "manifest.webmanifest",
        ] {
            assert!(is_known_asset(Path::new(asset)), "{asset}");
        }
        // A dotted client-side route (e.g. /v2.0) is not an asset type.
        assert!(!is_known_asset(Path::new("v2.0")));
        assert!(!is_known_asset(Path::new("dashboard")));
    }

    /// Create a throwaway `dist` dir (unique per test name) with `index.html`
    /// and `assets/app.js`, leaking a `StaticRouteInfo` that points at it.
    fn fixture(name: &str) -> StaticRouteInfo {
        let dir =
            std::env::temp_dir().join(format!("flatbed-static-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), b"<!doctype html><h1>shell</h1>").unwrap();
        std::fs::write(dir.join("assets/app.js"), b"console.log(1)").unwrap();
        StaticRouteInfo {
            mount: "/",
            dir: Box::leak(dir.to_str().unwrap().to_string().into_boxed_str()),
            fallback: Some("index.html"),
        }
    }

    #[tokio::test]
    async fn serves_existing_asset_with_type() {
        let route = fixture("asset");
        let parts = serve_one(&route, "/assets/app.js").await.expect("hit");
        assert_eq!(parts.body, b"console.log(1)");
        assert_eq!(parts.content_type, "text/javascript; charset=utf-8");
    }

    #[tokio::test]
    async fn root_serves_fallback_shell() {
        let route = fixture("root");
        let parts = serve_one(&route, "/").await.expect("fallback");
        assert_eq!(parts.content_type, "text/html; charset=utf-8");
        assert!(parts.body.starts_with(b"<!doctype"));
    }

    #[tokio::test]
    async fn extensionless_miss_serves_fallback() {
        let route = fixture("nav");
        let parts = serve_one(&route, "/dashboard").await.expect("spa fallback");
        assert_eq!(parts.content_type, "text/html; charset=utf-8");
    }

    #[tokio::test]
    async fn missing_asset_is_404_not_shell() {
        let route = fixture("missasset");
        assert!(serve_one(&route, "/assets/missing-x9.js").await.is_none());
    }

    #[tokio::test]
    async fn dotted_client_route_serves_fallback() {
        let route = fixture("dotted");
        // `/v2.0` looks like it has an extension but isn't an asset type.
        let parts = serve_one(&route, "/v2.0").await.expect("spa fallback");
        assert_eq!(parts.content_type, "text/html; charset=utf-8");
    }

    #[tokio::test]
    async fn traversal_is_rejected() {
        let route = fixture("traversal");
        assert!(serve_one(&route, "/../../etc/hosts").await.is_none());
    }

    #[tokio::test]
    async fn serve_first_matching_mount_wins() {
        let base = std::env::temp_dir();
        let make = |suffix: &str, body: &[u8]| -> &'static str {
            let dir = base.join(format!("flatbed-static-{}-{suffix}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("app.js"), body).unwrap();
            Box::leak(dir.to_str().unwrap().to_string().into_boxed_str())
        };
        let routes = [
            StaticRouteInfo {
                mount: "/",
                dir: make("winA", b"first"),
                fallback: None,
            },
            StaticRouteInfo {
                mount: "/",
                dir: make("winB", b"second"),
                fallback: None,
            },
        ];
        let parts = serve(&routes, "/app.js").await.expect("hit");
        assert_eq!(parts.body, b"first");
    }

    #[tokio::test]
    async fn no_fallback_extensionless_is_404() {
        // No fallback configured: an extensionless miss has nowhere to go.
        let route = StaticRouteInfo {
            mount: "/",
            dir: "/nonexistent",
            fallback: None,
        };
        assert!(serve_one(&route, "/dashboard").await.is_none());
    }

    #[tokio::test]
    async fn missing_fallback_file_is_404() {
        // `dir` exists but the configured fallback file does not.
        let dir = std::env::temp_dir().join(format!("flatbed-static-{}-nofb", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let route = StaticRouteInfo {
            mount: "/",
            dir: Box::leak(dir.to_str().unwrap().to_string().into_boxed_str()),
            fallback: Some("index.html"),
        };
        assert!(serve_one(&route, "/dashboard").await.is_none());
    }
}
