//! LAN-IP autodetection & ranking (SPEC §2.3, §12.1).

use std::net::Ipv4Addr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfKind {
    Lan,
    Vpn,
    Virtual,
}

impl IfKind {
    pub fn tag(self) -> &'static str {
        match self {
            IfKind::Lan => "LAN",
            IfKind::Vpn => "VPN",
            IfKind::Virtual => "virtual",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub ip: Ipv4Addr,
    pub kind: IfKind,
    pub score: i32,
}

/// Enumerate private IPv4 candidates, ranked best-first.
pub fn candidates() -> Vec<Candidate> {
    let mut list: Vec<Candidate> = Vec::new();

    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (name, ip) in ifaces {
            let std::net::IpAddr::V4(v4) = ip else {
                continue;
            };
            if !v4.is_private() || v4.is_loopback() {
                continue;
            }
            let kind = classify(&name, &v4);
            let score = score(&v4, kind);
            list.push(Candidate {
                name,
                ip: v4,
                kind,
                score,
            });
        }
    }
    // NOTE: PREFERRED_IP / --ip are honored upstream in `tui::choose_ip`, which
    // short-circuits before this list is even shown — so there's no override
    // injection here.

    list.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.ip.octets().cmp(&b.ip.octets()))
    });
    list
}

fn classify(name: &str, ip: &Ipv4Addr) -> IfKind {
    let n = name.to_lowercase();
    let virtual_hint = [
        "virtualbox",
        "vmware",
        "vethernet",
        "hyper-v",
        "vbox",
        "default switch",
        "docker",
    ];
    let vpn_hint = [
        "vpn",
        "wireguard",
        "wg",
        "tun",
        "tap",
        "openvpn",
        "proton",
        "tailscale",
        "zerotier",
    ];

    // VirtualBox host-only network (SPEC §2.3) is virtual.
    let o = ip.octets();
    if o[0] == 192 && o[1] == 168 && o[2] == 56 {
        return IfKind::Virtual;
    }
    if vpn_hint.iter().any(|h| n.contains(h)) {
        return IfKind::Vpn;
    }
    if virtual_hint.iter().any(|h| n.contains(h)) {
        return IfKind::Virtual;
    }
    IfKind::Lan
}

/// Higher is better. 192.168.* > 172.* > 10.*; VirtualBox host-only penalized;
/// VPN/virtual interfaces penalized (SPEC §2.3).
fn score(ip: &Ipv4Addr, kind: IfKind) -> i32 {
    let o = ip.octets();
    let mut s = match (o[0], o[1]) {
        (192, 168) => 300,
        (172, _) => 200,
        (10, _) => 100,
        _ => 50,
    };
    if o[0] == 192 && o[1] == 168 && o[2] == 56 {
        s -= 500; // VirtualBox host-only
    }
    match kind {
        IfKind::Lan => {}
        IfKind::Vpn => s -= 400,
        IfKind::Virtual => s -= 350,
    }
    s
}
