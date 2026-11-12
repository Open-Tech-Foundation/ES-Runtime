//! The media type a file is served as, by its extension — one table for every
//! place that needs one: `runtime:fs`'s `file().type` (and so the
//! `Content-Type` of `new Response(file(path))`), and esdev's dev server and
//! build.
//!
//! Short and explicit rather than a table of every type there is: what a server
//! hands a browser is what a build wrote, and a build writes JavaScript,
//! documents, stylesheets, and whatever the author put in `public`.

/// The media type for `path`'s extension, or `None` when it is not one this
/// table knows. What to do with an unknown file is the caller's decision: a
/// `Blob` reports `""`, and a dev server sends `application/octet-stream`.
pub fn content_type(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() {
        // `.env`, `.gitignore`: a dotfile's name is not an extension.
        return None;
    }
    Some(match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "wasm" => "application/wasm",
        "txt" | "map" => "text/plain; charset=utf-8",
        // A site's feeds and sitemap: a browser displays these rather than
        // downloading them, which is what a reader following the link expects.
        "xml" => "application/xml",
        "rss" => "application/rss+xml",
        "atom" => "application/atom+xml",
        "webmanifest" => "application/manifest+json",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "pdf" => "application/pdf",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_extension_names_its_type_in_any_case() {
        assert_eq!(
            content_type("public/index.html"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            content_type("dist/assets/main-abc.JS"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(content_type(r"C:\app\logo.png"), Some("image/png"));
        assert_eq!(content_type("photo.jpeg"), Some("image/jpeg"));
    }

    /// What a static site's `public/` holds besides pages and images: feeds,
    /// a sitemap, a manifest, documents, fonts and media.
    #[test]
    fn a_sites_public_files_have_types() {
        for (path, expected) in [
            ("rss.xml", "application/xml"),
            ("sitemap.xml", "application/xml"),
            ("feed.rss", "application/rss+xml"),
            ("feed.atom", "application/atom+xml"),
            ("site.webmanifest", "application/manifest+json"),
            ("llms.md", "text/markdown; charset=utf-8"),
            ("data.csv", "text/csv; charset=utf-8"),
            ("guide.pdf", "application/pdf"),
            ("Inter.ttf", "font/ttf"),
            ("Inter.otf", "font/otf"),
            ("intro.mp4", "video/mp4"),
            ("intro.webm", "video/webm"),
            ("ping.mp3", "audio/mpeg"),
            ("ping.ogg", "audio/ogg"),
            ("ping.wav", "audio/wav"),
        ] {
            assert_eq!(content_type(path), Some(expected), "{path}");
        }
    }

    #[test]
    fn an_unknown_or_missing_extension_has_no_type() {
        assert_eq!(content_type("data.bin"), None);
        assert_eq!(content_type("Makefile"), None);
        assert_eq!(content_type(".env"), None);
        assert_eq!(content_type("public/.htaccess"), None);
        // The directory's dot is not the file's.
        assert_eq!(content_type("v1.2/README"), None);
    }
}
