//! Self-signed certificate generation (pure Rust via `rcgen`) and the rustls
//! server config. The cert is cached on disk and reissued when the chosen
//! LAN-IP changes (SPEC §2.2).

use std::fs;
use std::io::BufReader;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DnType, Ia5String, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use time::{Duration, OffsetDateTime};

use crate::config;

fn dir() -> PathBuf {
    PathBuf::from(config::CERT_DIR)
}

struct Pems {
    cert: String,
    key: String,
}

/// Load a cached cert/key for `ip`, or generate (and cache) a fresh one.
fn load_or_generate(ip: Ipv4Addr) -> Result<Pems> {
    let d = dir();
    let cert_path = d.join("cert.pem");
    let key_path = d.join("key.pem");
    let ip_path = d.join("ip.txt");

    let cached_ip = fs::read_to_string(&ip_path).ok().map(|s| s.trim().to_string());
    if cached_ip.as_deref() == Some(&ip.to_string())
        && cert_path.exists()
        && key_path.exists()
    {
        let cert = fs::read_to_string(&cert_path)?;
        let key = fs::read_to_string(&key_path)?;
        tracing::info!("using cached certificate for {ip}");
        return Ok(Pems { cert, key });
    }

    tracing::info!("generating self-signed certificate for {ip}");
    let pems = generate(ip)?;

    fs::create_dir_all(&d).ok();
    let _ = fs::write(&cert_path, &pems.cert);
    let _ = fs::write(&key_path, &pems.key);
    let _ = fs::write(&ip_path, ip.to_string());
    Ok(pems)
}

fn generate(ip: Ipv4Addr) -> Result<Pems> {
    let mut params = CertificateParams::new(Vec::<String>::new())
        .context("building certificate params")?;

    params.subject_alt_names = vec![
        SanType::IpAddress(std::net::IpAddr::V4(ip)),
        SanType::IpAddress(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)),
        SanType::DnsName(Ia5String::try_from("localhost").unwrap()),
    ];
    params
        .distinguished_name
        .push(DnType::CommonName, "mousee");

    // ~10 year validity (SPEC §2.2).
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(3650);

    let key_pair = KeyPair::generate().context("generating key pair")?;
    let cert = params.self_signed(&key_pair).context("self-signing certificate")?;

    Ok(Pems {
        cert: cert.pem(),
        key: key_pair.serialize_pem(),
    })
}

/// Build a rustls server config for the given LAN-IP.
pub fn server_config(ip: Ipv4Addr) -> Result<Arc<ServerConfig>> {
    let pems = load_or_generate(ip)?;

    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut BufReader::new(pems.cert.as_bytes()))
            .collect::<Result<_, _>>()
            .context("parsing certificate PEM")?;

    let key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut BufReader::new(pems.key.as_bytes()))
            .context("parsing key PEM")?
            .context("no private key found in PEM")?;

    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("building rustls server config")?;

    Ok(Arc::new(cfg))
}
