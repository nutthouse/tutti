use rust_embed::Embed;

#[derive(Embed)]
#[folder = "dashboard/"]
pub struct DashboardAssets;

/// Look up the MIME content-type for a file path based on its extension.
pub fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_index_html_exists() {
        let file = DashboardAssets::get("index.html");
        assert!(file.is_some(), "index.html should be embedded");
    }

    #[test]
    fn content_type_lookup() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("app.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for("style.css"), "text/css; charset=utf-8");
        assert_eq!(content_type_for("unknown"), "application/octet-stream");
    }
}
