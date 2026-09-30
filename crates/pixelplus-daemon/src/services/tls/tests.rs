//! Tests for the local CA, leaf issue / re-issue and name constraints.

use super::*;
use std::net::{Ipv4Addr, Ipv6Addr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn test_ca() -> Ca {
    create_ca(
        &ca_common_name("Maple Street Lights", "ab12cd34ef"),
        "pixel-pi",
        now(),
    )
    .unwrap()
}

/// Find `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn ca_has_critical_name_constraints_and_pathlen_0() {
    let ca = test_ca();
    // id-ce-nameConstraints (2.5.29.30), then BOOLEAN TRUE (critical).
    let at = find(&ca.cert_der, &[0x06, 0x03, 0x55, 0x1D, 0x1E]).expect("name constraints");
    assert_eq!(
        &ca.cert_der[at + 5..at + 8],
        &[0x01, 0x01, 0xFF],
        "critical"
    );
    // basicConstraints (2.5.29.19) with cA TRUE and pathLen 0.
    let bc = find(&ca.cert_der, &[0x06, 0x03, 0x55, 0x1D, 0x13]).expect("basic constraints");
    let rest = &ca.cert_der[bc..bc + 20];
    assert!(
        find(rest, &[0x01, 0x01, 0xFF, 0x02, 0x01, 0x00]).is_some(),
        "{rest:02X?}"
    );
    // The permitted DNS names are in there as IA5 strings.
    for d in ["local", "home.arpa", "pixel-pi"] {
        assert!(find(&ca.cert_der, d.as_bytes()).is_some(), "{d}");
    }
    assert!(ca
        .meta
        .common_name
        .starts_with("PixelPlus Local CA – Maple Street Lights – ab12"));
    assert_eq!(fingerprint(&ca.cert_der).len(), 32 * 3 - 1);
}

#[test]
fn names_are_filtered_to_what_the_ca_may_sign() {
    let dns = ca_dns_constraints("Garage-Pi");
    assert!(dns.contains(&"garage-pi".to_string()));
    let ips = [
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
        IpAddr::V4(Ipv4Addr::new(100, 101, 2, 3)),
        IpAddr::V6("fd12::5".parse::<Ipv6Addr>().unwrap()),
        IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap()),
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
    ];
    let extra = vec![
        "lights.home.arpa".to_string(),
        "lights.example.com".to_string(),
        "bad_name.local".to_string(),
    ];
    let names = desired_names("Garage-Pi", &ips, &extra, &dns);
    assert_eq!(
        names,
        vec![
            "garage-pi.local",
            "garage-pi.lan",
            "garage-pi",
            "localhost",
            "lights.home.arpa",
            "100.101.2.3",
            "192.168.1.20",
            "fd12::5",
        ]
    );
    // A dotted host name is never added to the CA's constraints.
    assert!(!ca_dns_constraints("pi.example.com").contains(&"pi.example.com".to_string()));
    // Security audit 2: a bare host name that could be a top-level domain
    // is never a constraint (it would let the CA sign `*.christmas`).
    for tld_like in [
        "christmas",
        "shop",
        "lights",
        "app",
        "xn--p1ai",
        "pixelplus",
    ] {
        let dns = ca_dns_constraints(tld_like);
        assert!(!dns.contains(&tld_like.to_string()), "{tld_like}");
        assert!(!permitted(&format!("bank.{tld_like}"), &dns));
        // The .local / .lan variants still work.
        assert!(desired_names(tld_like, &[], &[], &dns).contains(&format!("{tld_like}.local")));
    }
    assert!(ca_dns_constraints("lights2").contains(&"lights2".to_string()));
    assert!(permitted("::ffff:10.1.2.3", &dns));
    assert!(!permitted("local.evil.com", &dns));
    assert!(!permitted("evillocal", &dns));
}

#[test]
fn reissue_rules() {
    let ca = test_ca();
    let fp = fingerprint(&ca.cert_der);
    let names = vec!["pixelplus.local".to_string(), "192.168.1.20".to_string()];
    let leaf = issue_leaf(&ca, &names, now()).unwrap();
    assert_eq!(reissue_reason(&leaf.meta, &fp, &names, now()), None);
    // Fewer names needed (an address went away): keep it.
    assert_eq!(reissue_reason(&leaf.meta, &fp, &names[..1], now()), None);
    // New IP address.
    let moved = vec!["pixelplus.local".to_string(), "192.168.1.77".to_string()];
    assert!(reissue_reason(&leaf.meta, &fp, &moved, now()).is_some());
    // New host name.
    let renamed = vec!["garage.local".to_string()];
    assert!(reissue_reason(&leaf.meta, &fp, &renamed, now()).is_some());
    // 30 days before expiry.
    let later = now() + ChronoDuration::days(LEAF_DAYS - RENEW_BEFORE_DAYS);
    assert!(reissue_reason(&leaf.meta, &fp, &names, later).is_some());
    assert_eq!(
        reissue_reason(&leaf.meta, &fp, &names, now() + ChronoDuration::days(300)),
        None
    );
    // Another CA.
    assert!(reissue_reason(&leaf.meta, "00:11", &names, now()).is_some());
    // A clock stuck in 1970 (no RTC yet) never triggers churn.
    let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap();
    assert_eq!(reissue_reason(&leaf.meta, &fp, &names, epoch), None);
    // Validity: 397 days, never more than Chrome's 398.
    let nb = DateTime::parse_from_rfc3339(&leaf.meta.not_before).unwrap();
    let na = DateTime::parse_from_rfc3339(&leaf.meta.not_after).unwrap();
    assert_eq!((na - nb).num_days(), LEAF_DAYS);
}

#[test]
fn certificates_are_dated_sanely_without_a_clock() {
    let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap();
    let ca = create_ca("PixelPlus Local CA – t", "pixelplus", epoch).unwrap();
    assert!(ca.meta.created_at.starts_with("2026-01-01"));
    assert!(
        ca.meta.not_after.starts_with("2035-12-30"),
        "{}",
        ca.meta.not_after
    );
    let leaf = issue_leaf(&ca, &["pixelplus.local".into()], epoch).unwrap();
    assert!(leaf.meta.not_after.starts_with("2027-"));
}

#[test]
fn files_roundtrip_with_private_keys_0600() {
    let data = std::env::temp_dir().join(format!("pp-tls-{}", rand::random::<u64>()));
    let dir = tls_dir(&data);
    let ca = test_ca();
    save_ca(&dir, &ca).unwrap();
    let leaf = issue_leaf(&ca, &["pixelplus.local".into()], now()).unwrap();
    save_leaf(&dir, &leaf).unwrap();
    let ca2 = load_ca(&dir).unwrap().unwrap();
    assert_eq!(ca2.cert_der, ca.cert_der);
    assert_eq!(ca2.meta, ca.meta);
    let leaf2 = load_leaf(&dir).unwrap().unwrap();
    assert_eq!(leaf2.cert_der, leaf.cert_der);
    assert_eq!(leaf2.key_der, leaf.key_der);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in ["ca.key", "leaf.key"] {
            let mode = std::fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{f}");
        }
    }
    // A reloaded CA still signs leaves that chain to the same certificate.
    let leaf3 = issue_leaf(&ca2, &["pixelplus.local".into()], now()).unwrap();
    assert_eq!(leaf3.meta.ca_fingerprint, fingerprint(&ca.cert_der));
    // Transfer bundle (F10): export, import on another controller, same CA.
    let (k, c, j) = export_ca(&data).unwrap().unwrap();
    let other = std::env::temp_dir().join(format!("pp-tls-{}", rand::random::<u64>()));
    assert!(export_ca(&other).unwrap().is_none());
    import_ca(&other, &k, &c, &j).unwrap();
    assert_eq!(
        load_ca(&tls_dir(&other)).unwrap().unwrap().cert_der,
        ca.cert_der
    );
    // Damaged input is refused.
    assert!(import_ca(&other, "garbage", &c, &j).is_err());
    assert!(import_ca(&other, &k, &c, "{}").is_err());
    // Security audit 2: another CA's key with this certificate is refused
    // (phones trust the certificate; the key must be its key).
    let stranger = create_ca("PixelPlus Local CA – x", "pixel-pi", now()).unwrap();
    let e = import_ca(&other, &stranger.key.serialize_pem(), &c, &j).unwrap_err();
    assert!(format!("{e:#}").contains("doesn't match"), "{e:#}");
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn base64_roundtrip() {
    for n in 0..40usize {
        let data: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
        assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
    }
    assert_eq!(b64_encode(b"Man"), "TWFu");
    assert_eq!(b64_encode(b"Ma"), "TWE=");
}

fn server_config(ca: &Ca, leaf: &Leaf) -> Arc<rustls::ServerConfig> {
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(leaf.key_der.clone().into());
    let chain = vec![leaf.cert_der.clone().into(), ca.cert_der.clone().into()];
    Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap(),
    )
}

/// A TLS client trusting only `ca`: does a handshake with `server_name` succeed?
async fn handshake(ca: &Ca, leaf: &Leaf, server_name: &str) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config(ca, leaf));
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        if let Ok(mut s) = acceptor.accept(tcp).await {
            let mut b = [0u8; 4];
            let _ = s.read_exact(&mut b).await;
            let _ = s.write_all(b"pong").await;
            let _ = s.shutdown().await;
        }
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.cert_der.clone().into()).unwrap();
    let client = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from(server_name.to_string()).unwrap();
    let res = async {
        let mut s = connector
            .connect(name, tcp)
            .await
            .map_err(|e| e.to_string())?;
        s.write_all(b"ping").await.map_err(|e| e.to_string())?;
        let mut b = [0u8; 4];
        s.read_exact(&mut b).await.map_err(|e| e.to_string())?;
        assert_eq!(&b, b"pong");
        Ok(())
    }
    .await;
    server.abort();
    res
}

#[tokio::test]
async fn leaf_verifies_against_the_ca_by_name_and_address() {
    let ca = test_ca();
    let names = desired_names(
        "pixel-pi",
        &[IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))],
        &[],
        &ca.meta.permitted_dns,
    );
    let leaf = issue_leaf(&ca, &names, Utc::now()).unwrap();
    for n in ["pixel-pi.local", "pixel-pi", "192.168.1.20", "localhost"] {
        handshake(&ca, &leaf, n)
            .await
            .unwrap_or_else(|e| panic!("{n}: {e}"));
    }
    // A name the leaf doesn't carry fails.
    assert!(handshake(&ca, &leaf, "192.168.1.21").await.is_err());
}

/// Name constraints are enforced: even a certificate the CA signed for a
/// public name is rejected (a stolen CA key can't impersonate web sites).
#[tokio::test]
async fn name_constraints_block_public_names() {
    let ca = test_ca();
    for evil in ["www.example.com", "8.8.8.8", "shop.christmas"] {
        let leaf = issue_leaf(&ca, &[evil.to_string()], Utc::now()).unwrap();
        let err = handshake(&ca, &leaf, evil).await.expect_err(evil);
        assert!(err.contains("NameConstraintViolation"), "{evil}: {err}");
    }
}

#[tokio::test]
async fn resolver_swaps_leaf_without_restart() {
    let ca = test_ca();
    let tls = TlsState::default();
    assert!(tls.material().is_none());
    let mk = |names: &[&str]| {
        let leaf = issue_leaf(
            &ca,
            &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            Utc::now(),
        )
        .unwrap();
        Material {
            ca_der: ca.cert_der.clone(),
            ca_meta: ca.meta.clone(),
            ca_fingerprint: fingerprint(&ca.cert_der),
            leaf,
        }
    };
    tls.install(mk(&["one.local"])).unwrap();
    let first = tls.resolver.current.read().clone().unwrap();
    tls.install(mk(&["two.local"])).unwrap();
    let second = tls.resolver.current.read().clone().unwrap();
    assert_ne!(first.cert[0], second.cert[0]);
    assert_eq!(tls.material().unwrap().leaf.meta.names, vec!["two.local"]);
}
