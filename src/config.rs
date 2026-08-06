use std::{
    collections::HashMap,
    net::{AddrParseError, SocketAddr},
    path::PathBuf,
};

use thiserror::Error;
use url::Url;

const DEFAULT_BIND: &str = "127.0.0.1:3000";
const DEFAULT_DATA_DIR: &str = "./data";
const DEFAULT_WEB_DIST_DIR: &str = "./frontend/dist";
const DEFAULT_GATEWAY_URL: &str = "https://gateway.vdome.mts.ru/";
const DEFAULT_RESIDENT_URL: &str = "https://resident-rest.vdome.mts.ru/";
const DEFAULT_CALL_ENDPOINT: &str = "https://webrtc.domofon.mts.ru/ws";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub web_dist_dir: PathBuf,
    pub gateway: Url,
    pub resident: Url,
    pub call_endpoint: Url,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_iter(std::env::vars())
    }

    // This fallible constructor is a public configuration API; `FromIterator`
    // cannot represent its `Result` return type.
    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, K, V>(values: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let values: HashMap<String, String> = values
            .into_iter()
            .map(|(key, value)| (key.as_ref().to_owned(), value.as_ref().to_owned()))
            .collect();

        let bind = value_or_default(&values, "VDOME_BIND", DEFAULT_BIND)
            .parse()
            .map_err(ConfigError::InvalidBind)?;
        let data_dir = PathBuf::from(value_or_default(
            &values,
            "VDOME_DATA_DIR",
            DEFAULT_DATA_DIR,
        ));
        let web_dist_dir = PathBuf::from(value_or_default(
            &values,
            "VDOME_WEB_DIST_DIR",
            DEFAULT_WEB_DIST_DIR,
        ));
        let gateway = parse_base_url(
            value_or_default(&values, "VDOME_GATEWAY_URL", DEFAULT_GATEWAY_URL),
            "VDOME_GATEWAY_URL",
            DEFAULT_GATEWAY_URL,
        )?;
        let resident = parse_base_url(
            value_or_default(&values, "VDOME_RESIDENT_URL", DEFAULT_RESIDENT_URL),
            "VDOME_RESIDENT_URL",
            DEFAULT_RESIDENT_URL,
        )?;
        let call_endpoint = parse_call_endpoint(
            value_or_default(&values, "VDOME_CALL_ENDPOINT", DEFAULT_CALL_ENDPOINT),
            "VDOME_CALL_ENDPOINT",
            DEFAULT_CALL_ENDPOINT,
        )?;

        Ok(Self {
            bind,
            data_dir,
            web_dist_dir,
            gateway,
            resident,
            call_endpoint,
        })
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid VDOME_BIND: {0}")]
    InvalidBind(#[source] AddrParseError),
    #[error("invalid provider URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("provider URL must use https")]
    ProviderUrlMustUseHttps,
    #[error("{setting} must be exactly {expected}")]
    UnapprovedProviderUrl {
        setting: &'static str,
        expected: &'static str,
    },
    #[error("call endpoint must have exact /ws path and no query or fragment")]
    InvalidCallEndpoint,
}

fn value_or_default<'a>(
    values: &'a HashMap<String, String>,
    key: &str,
    default: &'a str,
) -> &'a str {
    values.get(key).map_or(default, String::as_str)
}

fn parse_base_url(
    value: &str,
    setting: &'static str,
    expected: &'static str,
) -> Result<Url, ConfigError> {
    let mut url = parse_https_url(value)?;
    if !url.path().ends_with('/') {
        let mut path = url.path().to_owned();
        path.push('/');
        url.set_path(&path);
    }
    require_exact_url(url, setting, expected)
}

fn parse_call_endpoint(
    value: &str,
    setting: &'static str,
    expected: &'static str,
) -> Result<Url, ConfigError> {
    let url = parse_https_url(value)?;
    if url.path() != "/ws" || url.query().is_some() || url.fragment().is_some() {
        return Err(ConfigError::InvalidCallEndpoint);
    }
    require_exact_url(url, setting, expected)
}

fn parse_https_url(value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value)?;
    if url.scheme() != "https" {
        return Err(ConfigError::ProviderUrlMustUseHttps);
    }
    Ok(url)
}

fn require_exact_url(
    url: Url,
    setting: &'static str,
    expected: &'static str,
) -> Result<Url, ConfigError> {
    if url.as_str() != expected {
        return Err(ConfigError::UnapprovedProviderUrl { setting, expected });
    }
    Ok(url)
}
