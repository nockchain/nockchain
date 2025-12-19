use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, Order, OrderStatus,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use serde_json;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub struct AcmeManager {
    account: Account,
    domain: String,
    cache_dir: PathBuf,
    http_challenges: Arc<RwLock<HashMap<String, String>>>,
}

impl AcmeManager {
    pub async fn new(domain: String, email: String, cache_dir: PathBuf) -> Result<Self> {
        // Install default crypto provider for rustls
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        fs::create_dir_all(&cache_dir).await?;

        let account_key_path = cache_dir.join("account.key");
        let account = if account_key_path.exists() {
            info!("Loading existing ACME account");
            let serialized = fs::read_to_string(&account_key_path).await?;
            let credentials: AccountCredentials = serde_json::from_str(&serialized)?;
            let account = Account::from_credentials(credentials).await?;
            info!("Loaded existing ACME account");
            account
        } else {
            info!("Creating new ACME account for {}", email);
            let (account, credentials) = Account::create(
                &NewAccount {
                    contact: &[&format!("mailto:{}", email)],
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                LetsEncrypt::Production.url(),
                None,
            )
            .await?;

            // AccountCredentials doesn't have a to_pem method, let's serialize it differently
            let serialized = serde_json::to_string(&credentials)?;
            fs::write(&account_key_path, &serialized).await?;
            info!("ACME account created and saved");
            account
        };

        Ok(Self {
            account,
            domain,
            cache_dir,
            http_challenges: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn get_certificate(&self) -> Result<ServerConfig> {
        let cert_path = self.cache_dir.join("cert.pem");
        let key_path = self.cache_dir.join("key.pem");

        if cert_path.exists() && key_path.exists() {
            if let Ok(config) = self.load_existing_certificate(&cert_path, &key_path).await {
                if self.certificate_is_valid(&cert_path).await? {
                    info!("Using existing valid certificate");
                    return Ok(config);
                } else {
                    warn!("Existing certificate is expired or invalid, requesting new one");
                }
            }
        }

        info!("Requesting new certificate from Let's Encrypt");
        self.request_new_certificate().await
    }

    async fn load_existing_certificate(
        &self,
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<ServerConfig> {
        let cert_pem = fs::read_to_string(cert_path).await?;
        let key_pem = fs::read_to_string(key_path).await?;

        let cert_chain: Vec<CertificateDer> =
            CertificateDer::pem_reader_iter(&mut cert_pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()?;

        let private_key: PrivateKeyDer = PrivateKeyDer::from_pem_reader(&mut key_pem.as_bytes())?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)?;

        Ok(config)
    }

    async fn certificate_is_valid(&self, cert_path: &Path) -> Result<bool> {
        let cert_pem = fs::read_to_string(cert_path).await?;
        let certs: Vec<CertificateDer> = CertificateDer::pem_reader_iter(&mut cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()?;

        if let Some(cert_der) = certs.first() {
            let cert = x509_parser::parse_x509_certificate(cert_der.as_ref())?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;

            // Check if certificate expires within 30 days
            let expires_in_30_days = cert.1.validity().not_after.timestamp() - now < 30 * 24 * 3600;

            Ok(!expires_in_30_days)
        } else {
            Ok(false)
        }
    }

    async fn request_new_certificate(&self) -> Result<ServerConfig> {
        let identifier = Identifier::Dns(self.domain.clone());
        let mut order = self
            .account
            .new_order(&NewOrder {
                identifiers: &[identifier],
            })
            .await?;

        debug!("Created order");

        // Process challenges
        self.process_challenges(&mut order).await?;

        // Wait for order to be ready
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            order.refresh().await?;

            match order.state().status {
                OrderStatus::Ready => {
                    info!("Order ready, finalizing certificate");
                    break;
                }
                OrderStatus::Invalid => {
                    return Err(anyhow::anyhow!("Order became invalid"));
                }
                OrderStatus::Pending => {
                    debug!("Order still pending...");
                    continue;
                }
                _ => {
                    debug!("Order status: {:?}", order.state().status);
                }
            }
        }

        // Generate key pair and CSR
        let key_pair = rcgen::KeyPair::generate()?;

        // Create certificate parameters with explicit configuration
        let mut params = rcgen::CertificateParams::default();

        // Set the subject alternative names (this is what Let's Encrypt actually validates)
        params.subject_alt_names = vec![rcgen::SanType::DnsName(self.domain.clone().try_into()?)];

        // Set a proper distinguished name to avoid default "rcgen self signed cert"
        let mut distinguished_name = rcgen::DistinguishedName::new();
        distinguished_name.push(rcgen::DnType::CommonName, &self.domain);
        params.distinguished_name = distinguished_name;

        debug!("Generating CSR for domain: {}", self.domain);
        debug!("Subject alt names: {:?}", params.subject_alt_names);
        debug!("Distinguished name: {:?}", params.distinguished_name);

        let csr = params.serialize_request(&key_pair)?;

        // Finalize order
        info!("Finalizing order with CSR");
        order.finalize(csr.der()).await?;
        info!("Order finalized, waiting for certificate");

        // Wait for certificate with timeout
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 24; // 2 minutes total (5s * 24 = 120s)

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            order.refresh().await?;
            attempts += 1;

            match order.state().status {
                OrderStatus::Valid => {
                    info!("Order is valid, certificate ready");
                    break;
                }
                OrderStatus::Processing => {
                    info!("Order processing, attempt {}/{}", attempts, MAX_ATTEMPTS);
                }
                OrderStatus::Invalid => {
                    return Err(anyhow::anyhow!(
                        "Order became invalid during certificate generation"
                    ));
                }
                _ => {
                    info!(
                        "Order status: {:?}, attempt {}/{}",
                        order.state().status,
                        attempts,
                        MAX_ATTEMPTS
                    );
                }
            }

            if attempts >= MAX_ATTEMPTS {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for certificate after {} attempts. Final status: {:?}",
                    MAX_ATTEMPTS,
                    order.state().status
                ));
            }
        }

        info!("Attempting to retrieve certificate from order");
        let cert_chain_pem = order
            .certificate()
            .await?
            .ok_or_else(|| anyhow::anyhow!("Certificate not available after order completion"))?;

        info!("Certificate retrieved successfully, generating private key");
        let key_pem = key_pair.serialize_pem();

        // Save certificate and key
        fs::write(self.cache_dir.join("cert.pem"), &cert_chain_pem).await?;
        fs::write(self.cache_dir.join("key.pem"), &key_pem).await?;

        info!("Certificate saved successfully");

        // Build rustls config
        let cert_chain: Vec<CertificateDer> =
            CertificateDer::pem_reader_iter(&mut cert_chain_pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()?;

        let private_key: PrivateKeyDer = PrivateKeyDer::from_pem_reader(&mut key_pem.as_bytes())?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)?;

        Ok(config)
    }

    async fn process_challenges(&self, order: &mut Order) -> Result<()> {
        let authorizations = order.authorizations().await?;

        for authz in authorizations {
            match authz.status {
                AuthorizationStatus::Pending => {
                    let challenge = authz
                        .challenges
                        .iter()
                        .find(|c| c.r#type == ChallengeType::Http01)
                        .ok_or_else(|| anyhow::anyhow!("No HTTP-01 challenge found"))?;

                    let key_authorization = order.key_authorization(challenge);

                    // Store challenge response
                    {
                        let mut challenges = self.http_challenges.write().await;
                        challenges.insert(
                            challenge.token.clone(),
                            key_authorization.as_str().to_string(),
                        );
                    }

                    info!("Starting HTTP-01 challenge for {}", self.domain);
                    debug!("Challenge token: {}", challenge.token);

                    // Set challenge ready
                    order.set_challenge_ready(&challenge.url).await?;

                    // Wait for challenge validation - simplified
                    tokio::time::sleep(Duration::from_secs(10)).await;

                    // Clean up challenge
                    {
                        let mut challenges = self.http_challenges.write().await;
                        challenges.remove(&challenge.token);
                    }
                }
                AuthorizationStatus::Valid => {
                    debug!("Authorization already valid");
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Authorization in unexpected state: {:?}", authz.status
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn get_challenge_handler(&self) -> Arc<RwLock<HashMap<String, String>>> {
        self.http_challenges.clone()
    }
}
