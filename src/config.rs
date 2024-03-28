use quinn::ClientConfig;
use rustls::{Certificate, PrivateKey};
use std::{
    fs::File,
    io::{self, BufReader},
    path::Path,
    sync::Arc,
};

pub fn read_certs_from_file(
    cert_filename: &Path,
    key_filename: &Path,
) -> io::Result<(Vec<Certificate>, PrivateKey)> {
    let mut cert_chain_reader = BufReader::new(File::open(cert_filename)?);
    let certs = rustls_pemfile::certs(&mut cert_chain_reader)?
        .into_iter()
        .map(Certificate)
        .collect();

    let mut key_reader = BufReader::new(File::open(key_filename)?);
    let mut keys = {
        let keys = rustls_pemfile::pkcs8_private_keys(&mut key_reader)?;
        if keys.is_empty() {
            rustls_pemfile::rsa_private_keys(&mut key_reader)?
        } else {
            keys
        }
    };

    assert_eq!(keys.len(), 1);
    let key = rustls::PrivateKey(keys.remove(0));

    Ok((certs, key))
}

pub struct SkipServerVerification;

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

pub fn configure_client_without_server_verification() -> ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();

    ClientConfig::new(Arc::new(crypto))
}
