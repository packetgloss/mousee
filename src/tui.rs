//! Interactive interface selection + QR code printing (SPEC §12).

use std::net::Ipv4Addr;

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Select};
use qrcode::render::unicode;
use qrcode::QrCode;

use crate::net::{self, Candidate};

/// Choose the LAN-IP to advertise. Honors `forced` / `non_interactive` for
/// headless use; otherwise shows an arrow-key list with the recommended entry
/// pre-selected (SPEC §12.1).
pub fn choose_ip(forced: Option<Ipv4Addr>, non_interactive: bool) -> Result<Ipv4Addr> {
    let cands = net::candidates();

    if let Some(ip) = forced {
        return Ok(ip);
    }
    if cands.is_empty() {
        tracing::warn!("no private IPv4 interfaces found; using 127.0.0.1");
        return Ok(Ipv4Addr::LOCALHOST);
    }

    // Recommended candidate is first (net::candidates sorts best-first).
    let recommended = cands[0].ip;

    if non_interactive || cands.len() == 1 {
        let c = &cands[0];
        println!("Using {} ({}, {})", c.ip, c.name, c.kind.tag());
        return Ok(recommended);
    }

    let labels: Vec<String> = cands
        .iter()
        .enumerate()
        .map(|(i, candidate)| label(candidate, i == 0))
        .collect();
    let pick = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select network interface for your phone")
        .items(&labels)
        .default(0)
        .interact_opt()?;

    match pick {
        Some(i) => Ok(cands[i].ip),
        None => {
            // Esc / Ctrl-C -> fall back to recommended.
            println!("No selection; using recommended {recommended}");
            Ok(recommended)
        }
    }
}

fn label(c: &Candidate, recommended: bool) -> String {
    let rec = if recommended { "  (recommended)" } else { "" };
    format!("{:<15} {:<22} [{}]{}", c.ip, c.name, c.kind.tag(), rec)
}

/// Print a QR code plus the URL and the self-signed-cert warning (SPEC §12.2).
pub fn print_qr(url: &str) {
    match QrCode::new(url.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<unicode::Dense1x2>()
                .dark_color(unicode::Dense1x2::Light)
                .light_color(unicode::Dense1x2::Dark)
                .quiet_zone(true)
                .build();
            println!("\n{rendered}");
        }
        Err(e) => tracing::warn!("could not render QR: {e}"),
    }

    println!("  Scan the QR with your phone camera, or open:");
    println!("      {url}\n");
    println!("  TLS uses mousee's local root CA. Until it is trusted, your phone");
    println!("  will show a security warning; the page explains permanent setup.\n");
}
