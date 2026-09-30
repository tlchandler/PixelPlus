//! Host facts the cluster needs: IP addresses, broadcast addresses, hostname,
//! and the board / Pi model this node runs on; socket options for timing
//! (DSCP / WMM priority, kernel receive timestamps).

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
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
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

/// Identifier of this hardware (F10 hardware history, retired-controller
/// detection): the board EEPROM serial (`PPX-…`), else `pi-<serial>` from
/// the Raspberry Pi's device tree / cpuinfo. `None` on a PC without either.
pub fn hardware_serial() -> Option<String> {
    if let Some(s) = crate::services::system::detection()
        .0
        .serial
        .filter(|s| valid_serial(s))
    {
        return Some(s);
    }
    static PI: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PI.get_or_init(|| {
        let dt = std::fs::read_to_string("/sys/firmware/devicetree/base/serial-number")
            .ok()
            .map(|s| s.trim_matches(char::from(0)).trim().to_string());
        let cpu = || {
            std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|t| {
                t.lines()
                    .find(|l| l.starts_with("Serial"))
                    .and_then(|l| l.split(':').nth(1))
                    .map(|s| s.trim().to_string())
            })
        };
        dt.filter(|s| !s.is_empty())
            .or_else(cpu)
            .map(|s| s.trim_start_matches('0').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .map(|s| format!("pi-{s}"))
            .filter(|s| valid_serial(s))
    })
    .clone()
}

/// A serial as it may appear in beacons and the show (short, printable).
pub fn valid_serial(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

// ---------------------------------------------------------------------------
// Socket options for timing
// ---------------------------------------------------------------------------

/// DSCP EF (46) in the IP TOS byte: "expedited forwarding". Linux maps it to
/// the Wi-Fi voice/video access category (WMM), so clock probes and sync
/// packets skip the best-effort queue on the uplink (and on the AP if it
/// honours DSCP, as most do).
pub const TOS_EF: u8 = 0xB8;
/// DSCP AF41 for overlay frames (live, but bulkier than timing packets).
pub const TOS_AF41: u8 = 0x88;
/// `SO_PRIORITY` for the cluster sockets (0–6 need no privileges).
pub const SOCKET_PRIORITY: i32 = 6;

/// Mark a socket's packets with `tos` (IPv4 TOS / IPv6 traffic class) and a
/// high `SO_PRIORITY`. Best effort: returns the first error.
#[cfg(target_os = "linux")]
pub fn set_priority(sock: &tokio::net::UdpSocket, tos: u8) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = sock.as_raw_fd();
    let v6 = sock.local_addr().map(|a| a.is_ipv6()).unwrap_or(false);
    let set = |level: libc::c_int, name: libc::c_int, value: libc::c_int| {
        // SAFETY: valid fd and a pointer to a c_int of the given size.
        let r = unsafe {
            libc::setsockopt(
                fd,
                level,
                name,
                (&value as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    let tos_result = if v6 {
        set(
            libc::IPPROTO_IPV6,
            libc::IPV6_TCLASS,
            libc::c_int::from(tos),
        )
    } else {
        set(libc::IPPROTO_IP, libc::IP_TOS, libc::c_int::from(tos))
    };
    let prio_result = set(libc::SOL_SOCKET, libc::SO_PRIORITY, SOCKET_PRIORITY);
    tos_result.and(prio_result)
}

#[cfg(not(target_os = "linux"))]
pub fn set_priority(_sock: &tokio::net::UdpSocket, _tos: u8) -> std::io::Result<()> {
    Ok(())
}

/// Ask the kernel to timestamp every received datagram (`SO_TIMESTAMPNS`,
/// CLOCK_REALTIME, taken in the network core as the packet arrives: free of
/// tokio wake-up, JSON parsing and HMAC time). Works on any interface,
/// including Wi-Fi. Returns whether it is enabled.
#[cfg(target_os = "linux")]
pub fn enable_rx_timestamps(sock: &tokio::net::UdpSocket) -> bool {
    use std::os::fd::AsRawFd;
    let on: libc::c_int = 1;
    // SAFETY: valid fd and a pointer to a c_int of the given size.
    let r = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TIMESTAMPNS,
            (&on as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    r == 0
}

#[cfg(not(target_os = "linux"))]
pub fn enable_rx_timestamps(_sock: &tokio::net::UdpSocket) -> bool {
    false
}

/// A received datagram: length, source, kernel receive time (ns since the
/// Unix epoch, CLOCK_REALTIME) if the kernel attached one.
pub type Received = (usize, SocketAddr, Option<i128>);

/// Receive one datagram with its kernel timestamp (see
/// [`enable_rx_timestamps`]); falls back to a plain receive elsewhere.
pub async fn recv_timestamped(
    sock: &tokio::net::UdpSocket,
    buf: &mut [u8],
) -> std::io::Result<Received> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let fd = sock.as_raw_fd();
        sock.async_io(tokio::io::Interest::READABLE, || recvmsg_ts(fd, buf))
            .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (n, src) = sock.recv_from(buf).await?;
        Ok((n, src, None))
    }
}

#[cfg(target_os = "linux")]
fn recvmsg_ts(fd: std::os::fd::RawFd, buf: &mut [u8]) -> std::io::Result<Received> {
    // SAFETY: zeroed POD structs; every pointer handed to recvmsg points at a
    // live local buffer of the stated length for the duration of the call.
    unsafe {
        let mut name: libc::sockaddr_storage = std::mem::zeroed();
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        // u64 array: cmsg data must be suitably aligned.
        let mut control = [0u64; 16];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_name = (&mut name as *mut libc::sockaddr_storage).cast();
        msg.msg_namelen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&control) as _;
        let n = libc::recvmsg(fd, &mut msg, libc::MSG_DONTWAIT);
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut stamp = None;
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            let c = &*cmsg;
            if c.cmsg_level == libc::SOL_SOCKET && c.cmsg_type == libc::SCM_TIMESTAMPNS {
                let ts: libc::timespec = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast());
                stamp = Some(i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec));
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
        let src = sockaddr_to_std(&name).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "unknown address family")
        })?;
        Ok((n as usize, src, stamp))
    }
}

#[cfg(target_os = "linux")]
fn sockaddr_to_std(ss: &libc::sockaddr_storage) -> Option<SocketAddr> {
    match libc::c_int::from(ss.ss_family) {
        libc::AF_INET => {
            // SAFETY: the family says this storage holds a sockaddr_in.
            let a: &libc::sockaddr_in = unsafe { &*(ss as *const _ as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
            Some(SocketAddr::new(IpAddr::V4(ip), u16::from_be(a.sin_port)))
        }
        libc::AF_INET6 => {
            // SAFETY: the family says this storage holds a sockaddr_in6.
            let a: &libc::sockaddr_in6 = unsafe { &*(ss as *const _ as *const libc::sockaddr_in6) };
            let ip = std::net::Ipv6Addr::from(a.sin6_addr.s6_addr);
            Some(SocketAddr::V6(std::net::SocketAddrV6::new(
                ip,
                u16::from_be(a.sin6_port),
                a.sin6_flowinfo,
                a.sin6_scope_id,
            )))
        }
        _ => None,
    }
}

/// Convert a kernel receive stamp (ns since the Unix epoch) to the cluster's
/// monotonic ms clock, given both clocks read "now". Stamps older than
/// 100 ms or in the future (a wall-clock step in between) are refused.
pub fn stamp_to_mono_ms(stamp_ns: i128, real_now_ns: i128, mono_now_ms: f64) -> Option<f64> {
    let age_ms = (real_now_ns - stamp_ns) as f64 / 1e6;
    (-1.0..=100.0)
        .contains(&age_ms)
        .then(|| mono_now_ms - age_ms.max(0.0))
}

/// Wall clock now in ns since the Unix epoch.
pub fn real_now_ns() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
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
    fn stamps_convert_to_the_monotonic_clock() {
        let real = 1_700_000_000_000_000_000i128;
        // Received 2.5 ms ago.
        assert_eq!(
            stamp_to_mono_ms(real - 2_500_000, real, 1000.0),
            Some(997.5)
        );
        // Wall clock stepped: refused.
        assert_eq!(stamp_to_mono_ms(real - 500_000_000, real, 1000.0), None);
        assert_eq!(stamp_to_mono_ms(real + 5_000_000, real, 1000.0), None);
        // Tiny negative ages are clock-read order: clamped to "now".
        assert_eq!(stamp_to_mono_ms(real + 100_000, real, 1000.0), Some(1000.0));
    }

    #[tokio::test]
    async fn kernel_timestamps_and_priority_on_loopback() {
        let a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let enabled = enable_rx_timestamps(&b);
        set_priority(&a, TOS_EF).unwrap();
        a.send_to(b"hello", b.local_addr().unwrap()).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, src, stamp) = recv_timestamped(&b, &mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello");
        assert_eq!(src, a.local_addr().unwrap());
        if cfg!(target_os = "linux") {
            assert!(enabled);
            let age = (real_now_ns() - stamp.expect("kernel stamp")) as f64 / 1e6;
            assert!((0.0..100.0).contains(&age), "stamp age {age} ms");
        }
    }

    #[test]
    fn loopback_route() {
        assert_eq!(
            local_ip_towards("127.0.0.1".parse().unwrap()),
            Some("127.0.0.1".parse().unwrap())
        );
    }
}
