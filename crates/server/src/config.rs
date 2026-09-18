//! Validated listener and public-origin configuration.

use std::env;
use std::net::{AddrParseError, SocketAddr};
use thiserror::Error;
use url::{Host, Url};

pub const PUBLIC_ORIGIN_ENV: &str = "CLUMSIES_PUBLIC_ORIGIN";
const DATABASE_URL_ENV: &str = "DATABASE_URL";
const SERVER_ADDR_ENV: &str = "CLUMSIES_SERVER_ADDR";
const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:8080";

pub(crate) struct ServerConfig {
    pub(crate) database_url: String,
    pub(crate) listen_addr: SocketAddr,
    pub(crate) public_origin: Option<PublicOrigin>,
}

impl ServerConfig {
    pub(crate) fn from_env() -> Result<Self, ServerConfigError> {
        let database_url =
            env::var(DATABASE_URL_ENV).map_err(|_| ServerConfigError::Missing(DATABASE_URL_ENV))?;
        let listen_addr: SocketAddr = env::var(SERVER_ADDR_ENV)
            .unwrap_or_else(|_| DEFAULT_SERVER_ADDR.to_owned())
            .parse()
            .map_err(ServerConfigError::InvalidListenAddress)?;

        let public_origin = match env::var(PUBLIC_ORIGIN_ENV).map_err(|_| {
            ServerConfigError::PublicOrigin(PublicOriginError::Missing(PUBLIC_ORIGIN_ENV))
        })? {
            value if value.trim().eq_ignore_ascii_case("auto") => {
                if !listen_addr.ip().is_loopback() {
                    return Err(ServerConfigError::PublicOrigin(
                        PublicOriginError::AutoRequiresLoopback,
                    ));
                }
                None
            }
            value => Some(PublicOrigin::parse(&value)?),
        };

        Ok(Self {
            database_url,
            listen_addr,
            public_origin,
        })
    }
}

#[derive(Debug, Error)]
pub(crate) enum ServerConfigError {
    #[error("{0} is required to start clumsies Server")]
    Missing(&'static str),
    #[error("{SERVER_ADDR_ENV} is invalid: {0}")]
    InvalidListenAddress(#[source] AddrParseError),
    #[error(transparent)]
    PublicOrigin(#[from] PublicOriginError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicOrigin {
    url: Url,
}

impl PublicOrigin {
    pub fn from_env() -> Result<Self, PublicOriginError> {
        let value = env::var(PUBLIC_ORIGIN_ENV)
            .map_err(|_| PublicOriginError::Missing(PUBLIC_ORIGIN_ENV))?;
        Self::parse(&value)
    }

    pub fn parse(value: &str) -> Result<Self, PublicOriginError> {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("null") {
            return Err(PublicOriginError::Missing(PUBLIC_ORIGIN_ENV));
        }

        let url = Url::parse(value).map_err(PublicOriginError::InvalidUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(PublicOriginError::UnsupportedScheme);
        }
        if url.host().is_none() {
            return Err(PublicOriginError::MissingHost);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(PublicOriginError::CredentialsNotAllowed);
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(PublicOriginError::OriginOnly);
        }
        if url.scheme() == "http" && !is_loopback(&url) {
            return Err(PublicOriginError::HttpsRequired);
        }

        Ok(Self { url })
    }

    pub(crate) fn for_loopback(listen_addr: SocketAddr) -> Result<Self, PublicOriginError> {
        if !listen_addr.ip().is_loopback() {
            return Err(PublicOriginError::AutoRequiresLoopback);
        }
        Self::parse(&format!("http://{listen_addr}"))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.url.as_str().trim_end_matches('/')
    }

    pub fn oidc_callback_url(&self) -> String {
        self.url
            .join("/login/oauth2/code/oidc")
            .expect("OIDC callback path is valid")
            .to_string()
    }

    pub fn secure_cookies(&self) -> bool {
        self.url.scheme() == "https"
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[derive(Debug, Error)]
pub enum PublicOriginError {
    #[error("{0} is required and must not be null")]
    Missing(&'static str),
    #[error("{PUBLIC_ORIGIN_ENV} is invalid: {0}")]
    InvalidUrl(url::ParseError),
    #[error("{PUBLIC_ORIGIN_ENV} must use http or https")]
    UnsupportedScheme,
    #[error("{PUBLIC_ORIGIN_ENV} must include a host")]
    MissingHost,
    #[error("{PUBLIC_ORIGIN_ENV} must not include credentials")]
    CredentialsNotAllowed,
    #[error("{PUBLIC_ORIGIN_ENV} must contain only an origin without a path, query, or fragment")]
    OriginOnly,
    #[error("{PUBLIC_ORIGIN_ENV} must use HTTPS unless it is a loopback development origin")]
    HttpsRequired,
    #[error("{PUBLIC_ORIGIN_ENV}=auto requires a loopback {SERVER_ADDR_ENV}")]
    AutoRequiresLoopback,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_origin_derives_server_callbacks() {
        let origin = PublicOrigin::parse("https://app.clumsies.ai").unwrap();

        assert_eq!(
            origin.oidc_callback_url(),
            "https://app.clumsies.ai/login/oauth2/code/oidc"
        );
        assert!(origin.secure_cookies());
    }

    #[test]
    fn loopback_http_origin_is_allowed_for_development() {
        let origin = PublicOrigin::parse("http://127.0.0.1:18080").unwrap();

        assert!(!origin.secure_cookies());
    }

    #[test]
    fn auto_origin_resolves_from_the_bound_loopback_address() {
        let origin = PublicOrigin::for_loopback("127.0.0.1:49152".parse().unwrap()).unwrap();

        assert_eq!(origin.as_str(), "http://127.0.0.1:49152");
    }

    #[test]
    fn auto_origin_rejects_a_non_loopback_listener() {
        let error = PublicOrigin::for_loopback("0.0.0.0:49152".parse().unwrap()).unwrap_err();

        assert!(matches!(error, PublicOriginError::AutoRequiresLoopback));
    }

    #[test]
    fn remote_http_origin_is_rejected() {
        let error = PublicOrigin::parse("http://memory.example.com").unwrap_err();

        assert!(matches!(error, PublicOriginError::HttpsRequired));
    }

    #[test]
    fn origin_with_path_is_rejected() {
        let error = PublicOrigin::parse("https://memory.example.com/admin").unwrap_err();

        assert!(matches!(error, PublicOriginError::OriginOnly));
    }
}
