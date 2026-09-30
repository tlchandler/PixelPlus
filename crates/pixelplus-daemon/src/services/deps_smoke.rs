//! Proves the crates added for the feature wave work with the chosen feature
//! sets (ring backend, no aws-lc): a name-constrained CA signing a leaf that
//! a rustls server accepts, AES-256-GCM, minisign parsing. (WS0; rustfft: core tests/deps_smoke.rs)

#[test]
fn name_constrained_ca_leaf_loads_into_rustls_ring() {
    use rcgen::{
        BasicConstraints, CertificateParams, CidrSubnet, GeneralSubtree, IsCa, KeyPair,
        NameConstraints, PKCS_ECDSA_P256_SHA256,
    };
    let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::DnsName("local".into()),
            GeneralSubtree::IpAddress(CidrSubnet::V4([192, 168, 0, 0], [255, 255, 0, 0])),
        ],
        excluded_subtrees: vec![],
    });
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let leaf = CertificateParams::new(vec!["pixelplus.local".into(), "192.168.1.40".into()])
        .unwrap()
        .signed_by(&leaf_key, &issuer)
        .unwrap();
    assert!(ca_cert.pem().contains("BEGIN CERTIFICATE"));

    let provider = std::sync::Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let der_key =
        tokio_rustls::rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap();
    let cfg = tokio_rustls::rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone(), ca_cert.der().clone()], der_key)
        .unwrap();
    let _acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(cfg));
}

#[test]
fn aes_gcm_and_minisign_are_usable() {
    use aes_gcm::aead::{Aead, KeyInit};
    let key = aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(&[7u8; 32]);
    let cipher = aes_gcm::Aes256Gcm::new(key);
    let nonce = aes_gcm::Nonce::from_slice(&[1u8; 12]);
    let ct = cipher.encrypt(nonce, b"transfer".as_ref()).unwrap();
    assert_eq!(cipher.decrypt(nonce, ct.as_ref()).unwrap(), b"transfer");

    // A malformed key is rejected, not a panic.
    assert!(minisign_verify::PublicKey::from_base64("not a key").is_err());
}
