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
