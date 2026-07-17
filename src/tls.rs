//! Self-signed certificate generation (pure Rust via `rcgen`) and the rustls
//! server config. A stable local CA is cached on disk; IP-specific server
//! certificates can then be renewed without invalidating trust on the phone.

use std::collections::BTreeSet;
use std::fs;
use std::io::BufReader;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, Ia5String, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use time::{Duration, OffsetDateTime};

use crate::{config, net};

const CACHE_VERSION: &str = "2";
const LEAF_VALIDITY_DAYS: i64 = 397;
const LEAF_RENEW_AFTER_DAYS: i64 = 365;

/// Per-user directory for the cached cert/key/ip.
///
/// On Windows this is `%LOCALAPPDATA%\mousee` (e.g.
/// `C:\Users\you\AppData\Local\mousee`); elsewhere it falls back to
/// `$XDG_DATA_HOME`/`$HOME` and finally the current directory.
fn dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
    } else {
        std::env::var_os("XDG_DATA_HOME").or_else(|| std::env::var_os("HOME"))
    };
    base.map(PathBuf::from)
        .unwrap_or_default()
        .join(config::CERT_DIR)
}

struct Pems {
    cert: String,
    key: String,
}

struct CaPems {
    cert: String,
    key: String,
}

fn write_cache(path: &std::path::Path, contents: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

fn ca_params() -> Result<CertificateParams> {
    let mut params =
        CertificateParams::new(Vec::<String>::new()).context("building root certificate params")?;
    params
        .distinguished_name
        .push(DnType::CommonName, "mousee local root CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365 * 20);
    Ok(params)
}

/// The root is never rotated automatically: once installed and trusted on an
/// iPhone it remains the stable identity behind all future LAN-IP leaves.
fn load_or_generate_ca(d: &std::path::Path) -> Result<CaPems> {
    let cert_path = d.join("ca-cert.pem");
    let key_path = d.join("ca-key.pem");
    match (cert_path.exists(), key_path.exists()) {
        (true, true) => {
            return Ok(CaPems {
                cert: fs::read_to_string(&cert_path)?,
                key: fs::read_to_string(&key_path)?,
            });
        }
        (true, false) | (false, true) => {
            anyhow::bail!(
                "incomplete local CA cache in {}: restore the missing file or remove both CA files explicitly",
                d.display()
            );
        }
        (false, false) => {}
    }

    tracing::info!("generating persistent local root CA");
    let key = KeyPair::generate().context("generating root CA key")?;
    let cert = ca_params()?
        .self_signed(&key)
        .context("self-signing root CA")?;
    let pems = CaPems {
        cert: cert.pem(),
        key: key.serialize_pem(),
    };
    fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    write_cache(&cert_path, &pems.cert)?;
    write_cache(&key_path, &pems.key)?;
    Ok(pems)
}

fn certificate_ips(active: Ipv4Addr) -> BTreeSet<Ipv4Addr> {
    let mut ips: BTreeSet<Ipv4Addr> = net::candidates().into_iter().map(|c| c.ip).collect();
    ips.insert(active);
    ips.insert(Ipv4Addr::LOCALHOST);
    ips
}

fn cached_leaf_is_usable(d: &std::path::Path, active: Ipv4Addr) -> bool {
    let version = fs::read_to_string(d.join("tls-version.txt")).unwrap_or_default();
    let ips = fs::read_to_string(d.join("ip.txt")).unwrap_or_default();
    let renew_at = fs::read_to_string(d.join("leaf-renew-at.txt"))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_default();
    version.trim() == CACHE_VERSION
        && ips.lines().any(|line| line.trim() == active.to_string())
        && renew_at > OffsetDateTime::now_utc().unix_timestamp()
        && d.join("cert.pem").exists()
        && d.join("key.pem").exists()
}

/// Load a CA-signed cert/key covering `ip`, or issue and cache a fresh leaf.
fn load_or_generate(ip: Ipv4Addr) -> Result<Pems> {
    let d = dir();
    load_or_generate_in(&d, ip)
}

fn load_or_generate_in(d: &std::path::Path, ip: Ipv4Addr) -> Result<Pems> {
    let cert_path = d.join("cert.pem");
    let key_path = d.join("key.pem");
    let ca = load_or_generate_ca(d)?;

    if cached_leaf_is_usable(d, ip) {
        let cert = fs::read_to_string(&cert_path)?;
        let key = fs::read_to_string(&key_path)?;
        tracing::info!("using cached CA-signed certificate for {ip}");
        return Ok(Pems { cert, key });
    }

    let ips = certificate_ips(ip);
    tracing::info!("issuing local certificate for {ip} ({} SAN IPs)", ips.len());
    let pems = generate_leaf(&ca, &ips)?;

    fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    write_cache(&cert_path, &pems.cert)?;
    write_cache(&key_path, &pems.key)?;
    let ip_list = ips
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    write_cache(&d.join("ip.txt"), format!("{ip_list}\n"))?;
    write_cache(&d.join("tls-version.txt"), format!("{CACHE_VERSION}\n"))?;
    let renew_at =
        (OffsetDateTime::now_utc() + Duration::days(LEAF_RENEW_AFTER_DAYS)).unix_timestamp();
    write_cache(&d.join("leaf-renew-at.txt"), format!("{renew_at}\n"))?;
    Ok(pems)
}

fn generate_leaf(ca: &CaPems, ips: &BTreeSet<Ipv4Addr>) -> Result<Pems> {
    let mut params = CertificateParams::new(Vec::<String>::new())
        .context("building server certificate params")?;

    params.subject_alt_names = ips
        .iter()
        .copied()
        .map(|ip| SanType::IpAddress(std::net::IpAddr::V4(ip)))
        .chain(std::iter::once(SanType::DnsName(
            Ia5String::try_from("localhost").unwrap(),
        )))
        .collect();
    params.distinguished_name.push(DnType::CommonName, "mousee");
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(LEAF_VALIDITY_DAYS);

    let ca_key = KeyPair::from_pem(&ca.key).context("parsing root CA key")?;
    // Rcgen only needs an issuer object with the persisted root's subject and
    // public key. Reconstructing it does not rotate or overwrite the root cert.
    let issuer = ca_params()?
        .self_signed(&ca_key)
        .context("loading root CA issuer")?;
    let key_pair = KeyPair::generate().context("generating server key")?;
    let cert = params
        .signed_by(&key_pair, &issuer, &ca_key)
        .context("signing server certificate with local root CA")?;

    Ok(Pems {
        cert: format!("{}\n{}", cert.pem(), ca.cert),
        key: key_pair.serialize_pem(),
    })
}

/// DER form of the stable root CA, served for optional one-time installation
/// on iOS. The private CA key never leaves the PC.
pub fn ca_der() -> Result<Vec<u8>> {
    let ca = load_or_generate_ca(&dir())?;
    let mut reader = BufReader::new(ca.cert.as_bytes());
    let mut certs = rustls_pemfile::certs(&mut reader);
    let cert = certs
        .next()
        .transpose()
        .context("parsing root CA PEM")?
        .context("root CA PEM is empty")?;
    Ok(cert.as_ref().to_vec())
}

/// Ensure a usable cert/key exists on disk for `ip`, generating and caching one
/// if needed. The launcher calls this purely to learn whether TLS is available
/// (and thus which scheme the QR should advertise); the server process then
/// builds the actual rustls config from the same on-disk cache via
/// [`server_config`]. This way TLS is set up once — we no longer build a whole
/// throwaway `ServerConfig` just to test it.
pub fn ensure_cert(ip: Ipv4Addr) -> Result<()> {
    load_or_generate(ip).map(|_| ())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_ca_survives_leaf_reissue() {
        let temp = tempfile::tempdir().unwrap();
        let first_ip = Ipv4Addr::new(192, 0, 2, 10);
        let second_ip = Ipv4Addr::new(192, 0, 2, 11);

        let first = load_or_generate_in(temp.path(), first_ip).unwrap();
        let root = fs::read(temp.path().join("ca-cert.pem")).unwrap();
        let cached = load_or_generate_in(temp.path(), first_ip).unwrap();
        assert_eq!(first.cert, cached.cert);

        let second = load_or_generate_in(temp.path(), second_ip).unwrap();
        assert_ne!(first.cert, second.cert);
        assert_eq!(root, fs::read(temp.path().join("ca-cert.pem")).unwrap());
        assert!(fs::read_to_string(temp.path().join("ip.txt"))
            .unwrap()
            .lines()
            .any(|ip| ip == second_ip.to_string()));
    }

    #[test]
    fn incomplete_root_cache_is_not_silently_rotated() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("ca-cert.pem"), b"existing trust anchor").unwrap();

        let error = load_or_generate_ca(temp.path()).err().unwrap().to_string();
        assert!(error.contains("incomplete local CA cache"));
        assert!(!temp.path().join("ca-key.pem").exists());
        assert_eq!(
            fs::read(temp.path().join("ca-cert.pem")).unwrap(),
            b"existing trust anchor"
        );
    }
}
