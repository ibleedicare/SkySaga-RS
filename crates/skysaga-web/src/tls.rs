//! HTTPS, for the 2017 client.
//!
//! Build 36731 picks its URL scheme **per RPC**, from a bool passed to the URL builder at
//! `0x00c0eec0`. The login RPC is registered secure, so a http-only server never gets past the
//! login screen — the client shows `SERVER ERROR / 404`. There is no global switch to turn
//! that off, which is why this exists rather than being a client-side setting.
//!
//! Both schemes are served at once, on two ports, so a 10414 and a 36731 client can be online
//! together:
//!
//! ```text
//! http  :5164   build 10414
//! https :5165   build 36731
//! ```
//!
//! **The client accepts a self-signed certificate** — verified against the C# emulator, no
//! handshake errors. The certificate is generated once and cached, because a certificate that
//! changed on every restart would be a new identity to the client each time.

use std::path::{Path, PathBuf};

use anyhow::Context;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use tracing::info;

/// Where the generated certificate is cached, relative to the working directory.
const CERT_FILE: &str = "skysaga-dev-cert.pem";
const KEY_FILE: &str = "skysaga-dev-key.pem";

/// Load the cached development certificate, generating it if it is not there yet.
///
/// `SKYSAGA_WEB_CERT` / `SKYSAGA_WEB_KEY` override both paths, for a real certificate.
pub async fn dev_certificate() -> anyhow::Result<RustlsConfig> {
    let cert: PathBuf = std::env::var("SKYSAGA_WEB_CERT")
        .unwrap_or_else(|_| CERT_FILE.to_owned())
        .into();
    let key: PathBuf = std::env::var("SKYSAGA_WEB_KEY")
        .unwrap_or_else(|_| KEY_FILE.to_owned())
        .into();

    if !cert.exists() || !key.exists() {
        generate(&cert, &key)?;
    }

    RustlsConfig::from_pem_file(&cert, &key)
        .await
        .with_context(|| format!("loading the certificate from {}", cert.display()))
}

/// A self-signed certificate for the addresses the client is told to connect to.
///
/// The names matter less than they would in a browser — the client does not verify — but
/// getting them right costs nothing and keeps the certificate usable if it ever does.
fn generate(cert_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    let public_ip = std::env::var("SKYSAGA_PUBLIC_IP").unwrap_or_else(|_| "127.0.0.1".to_owned());

    let mut names = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];

    if !names.contains(&public_ip) {
        names.push(public_ip);
    }

    let generated = rcgen::generate_simple_self_signed(names.clone())
        .context("generating a self-signed certificate")?;

    std::fs::write(cert_path, generated.cert.pem())
        .with_context(|| format!("writing {}", cert_path.display()))?;
    std::fs::write(key_path, generated.signing_key.serialize_pem())
        .with_context(|| format!("writing {}", key_path.display()))?;

    info!(
        path = %cert_path.display(),
        names = ?names,
        "generated a self-signed certificate",
    );

    Ok(())
}

/// Serve `router` over TLS on `addr`, forever.
pub async fn serve(addr: std::net::SocketAddr, router: Router) -> anyhow::Result<()> {
    let config = dev_certificate().await?;

    info!(%addr, "https listening (build 36731)");

    axum_server::bind_rustls(addr, config)
        .serve(router.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await
        .context("the https server failed")
}
