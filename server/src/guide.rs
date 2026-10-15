//! Serve the repository-built `VitePress` site from the same HTTPS origin.
use anyhow::{Result, ensure};
use axum::{Router, response::Redirect, routing::get};
use std::path::Path;
use tower_http::services::ServeDir;

pub fn router(web_root: &Path) -> Result<Router> {
    let site = web_root.join("guide");
    ensure!(
        site.join("index.html").is_file(),
        "VitePress documentation is missing from {}",
        site.display()
    );
    Ok(Router::new()
        .route("/guide", get(|| async { Redirect::temporary("/guide/") }))
        .nest_service("/guide/", ServeDir::new(site))
        .route("/manual", get(|| async { Redirect::temporary("/guide/") }))
        .route("/manual/", get(|| async { Redirect::temporary("/guide/") })))
}
