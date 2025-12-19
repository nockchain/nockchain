use clap::Args;
use http::Uri;
use nockapp::noun::slab::NounSlab;
use nockapp::NockAppError;
use nockapp_grpc::{private_nockapp, public_nockchain};
use tracing::info;

use crate::command::ClientType;
use crate::Wallet;

#[derive(Args, Debug, Clone)]
pub(crate) struct ConnectionCli {
    /// Which client to connect to: public or private
    #[arg(long, value_enum, default_value = "public", global = true)]
    pub client: ClientType,

    /// localhost port at which listener connects to private grpc server
    #[arg(long, default_value_t = 5555)]
    pub private_grpc_server_port: u16,

    /// Address of the public server (host[:port] or URI)
    #[arg(long, value_parser = GrpcEndpoint::parse, default_value = "https://nockchain-api.zorp.io", global = true)]
    pub public_grpc_server_addr: GrpcEndpoint,
}

impl ConnectionCli {
    pub(crate) fn target(&self) -> GrpcTarget {
        match self.client {
            ClientType::Public => GrpcTarget::Public {
                endpoint: self.public_grpc_server_addr.to_string(),
            },
            ClientType::Private => GrpcTarget::Private {
                endpoint: format!("http://127.0.0.1:{}", self.private_grpc_server_port),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GrpcEndpoint(String);

impl GrpcEndpoint {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("gRPC server address cannot be empty".to_string());
        }

        if trimmed.chars().any(|c| c.is_whitespace()) {
            return Err("gRPC server address must not contain spaces".to_string());
        }

        if trimmed.to_ascii_lowercase().starts_with("unix:") {
            return Err("unix socket endpoints are not supported".to_string());
        }

        let normalized = if trimmed.contains("://") {
            trimmed.to_string()
        } else {
            format!("http://{}", trimmed)
        };

        let uri: Uri = normalized
            .parse()
            .map_err(|e| format!("Invalid gRPC server address '{}': {}", normalized, e))?;

        let scheme = uri.scheme().ok_or_else(|| {
            format!(
                "gRPC server address '{}' is missing a URI scheme",
                normalized
            )
        })?;

        let scheme_str = scheme.as_str();
        if !matches!(scheme_str, "http" | "https") {
            return Err(format!(
                "Unsupported gRPC URI scheme '{}'; only http and https are supported",
                scheme_str
            ));
        }

        if uri.host().is_none() {
            return Err(format!(
                "gRPC server address '{}' is missing a host",
                normalized
            ));
        }

        let host = uri.host().expect("URI should have a host");
        let port = uri.port_u16().unwrap_or(match scheme_str {
            "https" => 443,
            _ => 80,
        });

        let final_endpoint = if uri.port_u16().is_some() {
            normalized
        } else {
            let path = uri.path();
            let path = if path.is_empty() || path == "/" {
                ""
            } else {
                path
            };

            let mut endpoint = format!("{}://{}:{}{}", scheme_str, host, port, path);
            if let Some(query) = uri.query() {
                endpoint.push('?');
                endpoint.push_str(query);
            }
            endpoint
        };

        Ok(Self(final_endpoint))
    }
}

impl std::fmt::Display for GrpcEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for GrpcEndpoint {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone)]
pub(crate) enum GrpcTarget {
    Public { endpoint: String },
    Private { endpoint: String },
}

impl GrpcTarget {
    fn label(&self) -> &'static str {
        match self {
            GrpcTarget::Public { .. } => "public",
            GrpcTarget::Private { .. } => "private",
        }
    }
}

pub(crate) async fn sync_wallet_balance(
    wallet: &mut Wallet,
    target: &GrpcTarget,
    pubkeys: Vec<String>,
    tracked_names: Vec<String>,
) -> Result<Vec<NounSlab>, NockAppError> {
    match target {
        GrpcTarget::Private { endpoint } => {
            let mut client = private_nockapp::PrivateNockAppGrpcClient::connect(endpoint.clone())
                .await
                .map_err(|err| connection_error(target.label(), endpoint, err))?;
            info!(endpoint = %endpoint, "Connected to private NockApp gRPC server");

            wallet
                .app
                .add_io_driver(private_nockapp::driver::grpc_listener_driver(
                    endpoint.clone(),
                ))
                .await;
            Wallet::update_balance_grpc_private(&mut client, pubkeys, tracked_names).await
        }
        GrpcTarget::Public { endpoint } => {
            let mut client = public_nockchain::PublicNockchainGrpcClient::connect(endpoint.clone())
                .await
                .map_err(|err| connection_error(target.label(), endpoint, err))?;
            info!(endpoint = %endpoint, "Connected to public NockApp gRPC server");

            wallet
                .app
                .add_io_driver(public_nockchain::v2::driver::grpc_listener_driver(
                    endpoint.clone(),
                ))
                .await;
            Wallet::update_balance_grpc_public(&mut client, pubkeys, tracked_names).await
        }
    }
}

fn connection_error<E: std::fmt::Display>(kind: &str, endpoint: &str, err: E) -> NockAppError {
    NockAppError::OtherError(format!(
        "Failed to connect to {kind} Nockchain gRPC server at {endpoint}: {err}. Double-check that the server is reachable and the address matches your configuration."
    ))
}

#[cfg(test)]
mod tests {
    use super::GrpcEndpoint;

    #[test]
    fn accepts_explicit_http_host_and_port() {
        let parsed = GrpcEndpoint::parse("http://example.com:8080").unwrap();
        assert_eq!(parsed.to_string(), "http://example.com:8080");
    }

    #[test]
    fn accepts_https_host_and_port() {
        let parsed = GrpcEndpoint::parse("https://example.com:4430").unwrap();
        assert_eq!(parsed.to_string(), "https://example.com:4430");
    }

    #[test]
    fn defaults_http_port_when_missing() {
        let parsed = GrpcEndpoint::parse("http://example.com").unwrap();
        assert_eq!(parsed.to_string(), "http://example.com:80");
    }

    #[test]
    fn defaults_https_port_when_missing() {
        let parsed = GrpcEndpoint::parse("https://secure.example.com").unwrap();
        assert_eq!(parsed.to_string(), "https://secure.example.com:443");
    }

    #[test]
    fn infers_scheme_for_plain_host_port() {
        let parsed = GrpcEndpoint::parse("127.0.0.1:9000").unwrap();
        assert_eq!(parsed.to_string(), "http://127.0.0.1:9000");
    }

    #[test]
    fn accepts_ipv6_with_brackets() {
        let parsed = GrpcEndpoint::parse("http://[2001:db8::1]:7000").unwrap();
        assert_eq!(parsed.to_string(), "http://[2001:db8::1]:7000");
    }

    #[test]
    fn rejects_spaces_in_address() {
        let err = GrpcEndpoint::parse("example.com :8080").unwrap_err();
        assert!(err.contains("spaces"));
    }

    #[test]
    fn rejects_unix_scheme() {
        let err = GrpcEndpoint::parse("unix:///tmp/nock.sock").unwrap_err();
        assert!(err.contains("not supported"));
    }
}
