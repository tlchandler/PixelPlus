//! Host facts the cluster needs: IP addresses, broadcast addresses, hostname,
//! and the board / Pi model this node runs on.

use pixelplus_core::model::BoardKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Non-loopback IPv4 addresses and their subnet broadcast addresses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Interfaces {
    pub ips: Vec<IpAddr>,
    pub broadcasts: Vec<Ipv4Addr>,
}

pub fn interfaces() -> Interfaces {
    let mut out = Interfaces::default();
    let Ok(list) = if_addrs::get_if_addrs() else {
        return out;
    };
    for iface in list {
        if iface.is_loopback() {
            continue;
        }
        if let if_addrs::IfAddr::V4(v4) = &iface.addr {
            if v4.ip.is_link_local() {
                continue;
            }
            if !out.ips.contains(&IpAddr::V4(v4.ip)) {
                out.ips.push(IpAddr::V4(v4.ip));
            }
            let bcast = v4.broadcast.unwrap_or_else(|| {
                let mask = u32::from(v4.netmask);
                Ipv4Addr::from(u32::from(v4.ip) | !mask)
            });
            if bcast != v4.ip && !out.broadcasts.contains(&bcast) {
                out.broadcasts.push(bcast);
            }
        }
    }
    out
}

/// `ip` is this host, or inside one of the subnets of its interfaces
/// (IPv4 by netmask, IPv6 link-local), i.e. on the local network segment.
pub fn on_local_subnet(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        other => other,
    };
    if ip.is_loopback() {
        return true;
    }
    let Ok(list) = if_addrs::get_if_addrs() else {
        return false;
    };
    list.iter().any(|iface| match (&iface.addr, ip) {
        (if_addrs::IfAddr::V4(v4), IpAddr::V4(peer)) => {
            let mask = u32::from(v4.netmask);
            u32::from(v4.ip) & mask == u32::from(peer) & mask
        }
        (if_addrs::IfAddr::V6(v6), IpAddr::V6(peer)) => {
            let mask = u128::from(v6.netmask);
            mask != 0 && u128::from(v6.ip) & mask == u128::from(peer) & mask
        }
        _ => false,
    })
}

/// The local address the OS would use to reach `peer` (no packet is sent).
pub fn local_ip_towards(peer: IpAddr) -> Option<IpAddr> {
    let bind: SocketAddr = match peer {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let s = std::net::UdpSocket::bind(bind).ok()?;
    s.connect((peer, 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// `http://ip:port` with IPv6 brackets.
pub fn http_url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "pixelplus".into())
}

/// "pixelplus-garage" → "Pixelplus Garage".
pub fn title_case(s: &str) -> String {
    s.split(|c: char| c == '-' || c == '_' || c == '.' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Board as chosen in the setup wizard or detected (shared with `GET /system`).
pub fn local_board(state: &crate::state::AppState) -> (BoardKind, Option<String>) {
    crate::services::system::effective_board(state)
}

/// Raspberry Pi model string, if running on a Pi (cached detection).
pub fn pi_model() -> Option<String> {
    crate::services::system::detection().1.map(|p| p.model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case_hostnames() {
        assert_eq!(title_case("pixelplus-garage"), "Pixelplus Garage");
        assert_eq!(title_case("mega_tree.local"), "Mega Tree Local");
        assert_eq!(title_case(""), "");
    }

    #[test]
    fn urls() {
        assert_eq!(
            http_url("10.0.0.2".parse().unwrap(), 80),
            "http://10.0.0.2:80"
        );
        assert_eq!(http_url("::1".parse().unwrap(), 8080), "http://[::1]:8080");
    }

    #[test]
    fn loopback_route() {
        assert_eq!(
            local_ip_towards("127.0.0.1".parse().unwrap()),
            Some("127.0.0.1".parse().unwrap())
        );
    }
}
