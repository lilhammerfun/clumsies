//! Validated deployment settings for the listener, database, identity provider, and setup.

use std::env;
use std::net::{AddrParseError, SocketAddr};
use thiserror::Error;
use url::{Host, Url};

/// Deployment setting for the externally reachable server origin.
pub const PUBLIC_ORIGIN_ENV: &str = "CLUMSIES_PUBLIC_ORIGIN";
/// Deployment setting supplying the PostgreSQL connection URL.
const DATABASE_URL_ENV: &str = "DATABASE_URL";
/// Deployment setting for the server's socket listen address.
const SERVER_ADDR_ENV: &str = "CLUMSIES_SERVER_ADDR";
/// Default local listener used when deployment provides no explicit address.
const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:8080";

/// Explicit startup configuration parsed before constructing external dependencies.
pub(crate) struct ServerConfig {
    /// PostgreSQL connection URL; it may contain credentials and must not be logged.
    pub(crate) database_url: String,
    /// Socket address on which the server listens; port zero requests a dynamic port.
    pub(crate) listen_addr: SocketAddr,
    /// Validated browser-visible server origin used for redirects and cookie security.
    pub(crate) public_origin: Option<PublicOrigin>,
    /// Explicit identity-provider configuration, absent when login is unconfigured.
    pub(crate) oidc: Option<OidcConfig>,
    /// Deployment-provided bootstrap secret; never expose it in responses or logs.
    pub(crate) setup_code: Option<String>,
}

impl ServerConfig {
    /// Parse deployment settings before any dependency construction or network access.
    ///
    /// # Errors
    /// Rejects missing required settings, invalid listen addresses, unsafe public origins, or
    /// incomplete OIDC configuration.
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
            oidc: OidcConfig::parse(
                optional_env("CLUMSIES_OIDC_ISSUER"),
                optional_env("CLUMSIES_OIDC_CLIENT_ID"),
                optional_env("CLUMSIES_OIDC_CLIENT_SECRET"),
                optional_env("CLUMSIES_CLIENT_REDIRECT_URIS"),
            )?,
            setup_code: optional_env("CLUMSIES_SETUP_CODE"),
        })
    }
}

/// Explicit identity-provider credentials and validated client redirect allowlist.
pub(crate) struct OidcConfig {
    /// Verified or configured OIDC issuer identifying the external identity authority.
    pub(crate) issuer: String,
    /// OIDC application identifier configured by the deployment.
    pub(crate) client_id: String,
    /// OIDC application credential; never expose it in responses or logs.
    pub(crate) client_secret: String,
    /// Explicitly permitted client callback URLs, including loopback callback templates.
    pub(crate) allowed_redirects: Vec<Url>,
}

impl OidcConfig {
    /// Validate an optional provider configuration and deduplicate explicit native callback URLs.
    ///
    /// # Errors
    /// Rejects partial or null provider credentials and malformed native callback URLs.
    fn parse(
        issuer: Option<String>,
        client_id: Option<String>,
        client_secret: Option<String>,
        redirects: Option<String>,
    ) -> Result<Option<Self>, ServerConfigError> {
        if issuer.is_none() && client_id.is_none() && client_secret.is_none() {
            return Ok(None);
        }
        let issuer = required_oidc_value("CLUMSIES_OIDC_ISSUER", issuer)?;
        let client_id = required_oidc_value("CLUMSIES_OIDC_CLIENT_ID", client_id)?;
        let client_secret = required_oidc_value("CLUMSIES_OIDC_CLIENT_SECRET", client_secret)?;
        let mut allowed_redirects = Vec::new();
        if let Some(redirects) = redirects {
            for value in redirects
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let redirect = Url::parse(value).map_err(|error| {
                    ServerConfigError::Oidc(format!("invalid client redirect URI {value}: {error}"))
                })?;
                if !allowed_redirects.contains(&redirect) {
                    allowed_redirects.push(redirect);
                }
            }
        }
        Ok(Some(Self {
            issuer,
            client_id,
            client_secret,
            allowed_redirects,
        }))
    }
}

/// Read and trim an optional deployment setting, treating blank values as absent.
fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Reject missing OIDC credentials and literal null placeholders before discovery.
///
/// # Errors
/// Rejects a missing credential or a literal null placeholder.
fn required_oidc_value(name: &str, value: Option<String>) -> Result<String, ServerConfigError> {
    match value {
        Some(value) if !value.eq_ignore_ascii_case("null") => Ok(value),
        _ => Err(ServerConfigError::Oidc(format!(
            "{name} is required and must not be null"
        ))),
    }
}

/// Missing or invalid deployment configuration preventing startup.
#[derive(Debug, Error)]
pub(crate) enum ServerConfigError {
    /// Provider settings are incomplete or contain invalid credential or redirect values.
    #[error("{0}")]
    Oidc(String),
    /// A required deployment setting is absent or blank.
    #[error("{0} is required to start clumsies Server")]
    Missing(&'static str),
    /// The listen setting cannot be parsed as a socket address.
    #[error("{SERVER_ADDR_ENV} is invalid: {0}")]
    InvalidListenAddress(#[source] AddrParseError),
    /// The configured public origin violates the callback transport or URL requirements.
    #[error(transparent)]
    PublicOrigin(#[from] PublicOriginError),
}

/// Validated HTTP origin whose transport is safe for browser redirects and cookies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicOrigin {
    /// Validated credential-free origin used to derive server callbacks.
    url: Url,
}

impl PublicOrigin {
    /// Read and validate the deployment's explicit external HTTP origin.
    ///
    /// # Errors
    /// Rejects an absent setting or any origin that fails the public callback URL contract.
    pub fn from_env() -> Result<Self, PublicOriginError> {
        let value = env::var(PUBLIC_ORIGIN_ENV)
            .map_err(|_| PublicOriginError::Missing(PUBLIC_ORIGIN_ENV))?;
        Self::parse(&value)
    }

    /// Accept only credential-free HTTP origins, requiring HTTPS outside loopback development.
    ///
    /// # Errors
    /// Rejects missing values, invalid URLs, user information, non-origin components, and
    /// insecure non-loopback transport.
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

    /// Resolve the public development origin from the actual bound loopback listener.
    ///
    /// # Errors
    /// Rejects non-loopback listener addresses or an origin that cannot be represented safely.
    pub(crate) fn for_loopback(listen_addr: SocketAddr) -> Result<Self, PublicOriginError> {
        if !listen_addr.ip().is_loopback() {
            return Err(PublicOriginError::AutoRequiresLoopback);
        }
        Self::parse(&format!("http://{listen_addr}"))
    }

    /// Return the stable string representation used by the surrounding persistence or protocol
    /// contract.
    pub(crate) fn as_str(&self) -> &str {
        self.url.as_str().trim_end_matches('/')
    }

    /// Derive the server's OIDC callback from its validated public origin.
    pub fn oidc_callback_url(&self) -> String {
        self.url
            .join("/login/oauth2/code/oidc")
            .expect("OIDC callback path is valid")
            .to_string()
    }

    /// Indicate whether the public origin requires HTTPS-only setup cookies.
    pub fn secure_cookies(&self) -> bool {
        self.url.scheme() == "https"
    }
}

/// Recognize localhost and loopback IP origins permitted to use development HTTP.
fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// Reasons an external server origin cannot safely be used for callbacks.
#[derive(Debug, Error)]
pub enum PublicOriginError {
    /// No explicit origin was supplied where automatic selection is unavailable.
    #[error("{0} is required and must not be null")]
    Missing(&'static str),
    /// The configured origin is not a valid absolute URL.
    #[error("{PUBLIC_ORIGIN_ENV} is invalid: {0}")]
    InvalidUrl(url::ParseError),
    /// The origin uses a scheme unsupported by browser callbacks.
    #[error("{PUBLIC_ORIGIN_ENV} must use http or https")]
    UnsupportedScheme,
    /// The origin URL has no usable host.
    #[error("{PUBLIC_ORIGIN_ENV} must include a host")]
    MissingHost,
    /// An origin contains user information that must never enter redirects.
    #[error("{PUBLIC_ORIGIN_ENV} must not include credentials")]
    CredentialsNotAllowed,
    /// The supplied URL contains a path, query, or fragment beyond an origin.
    #[error("{PUBLIC_ORIGIN_ENV} must contain only an origin without a path, query, or fragment")]
    OriginOnly,
    /// A non-loopback origin uses an insecure transport.
    #[error("{PUBLIC_ORIGIN_ENV} must use HTTPS unless it is a loopback development origin")]
    HttpsRequired,
    /// Automatic public-origin selection was requested for a non-loopback listener.
    #[error("{PUBLIC_ORIGIN_ENV}=auto requires a loopback {SERVER_ADDR_ENV}")]
    AutoRequiresLoopback,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oidc_configuration_is_optional_but_partial_credentials_are_rejected() {
        assert!(OidcConfig::parse(None, None, None, None).unwrap().is_none());
        assert!(matches!(
            OidcConfig::parse(
                Some("https://identity.example.test".to_owned()),
                None,
                None,
                None
            ),
            Err(ServerConfigError::Oidc(_))
        ));
        assert!(matches!(
            required_oidc_value("CLUMSIES_OIDC_CLIENT_ID", Some("null".to_owned())),
            Err(ServerConfigError::Oidc(_))
        ));
    }

    #[test]
    fn oidc_configuration_validates_and_deduplicates_redirects_without_io() {
        let config = OidcConfig::parse(
            Some("https://identity.example.test".to_owned()),
            Some("client".to_owned()),
            Some("secret".to_owned()),
            Some(" http://127.0.0.1/callback, ,http://127.0.0.1/callback ".to_owned()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            config.allowed_redirects,
            vec![Url::parse("http://127.0.0.1/callback").unwrap()]
        );
        assert!(matches!(
            OidcConfig::parse(
                Some(config.issuer),
                Some(config.client_id),
                Some(config.client_secret),
                Some("invalid redirect".to_owned()),
            ),
            Err(ServerConfigError::Oidc(_))
        ));
    }

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
