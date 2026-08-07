use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use reqwest::Method;
use serde::Deserialize;
use url::form_urlencoded;

use crate::error::{AppError, ErrorCode};

use super::{MtsClient, ProviderBase};

const EVENTS_PATH: &str = "/vdome-smart-facade/api/v1/event";
const MAX_PROVIDER_PAGE_SIZE: u32 = 1_000;
const MAX_PROVIDER_PAGES: u32 = 10_000;
const MAX_PROVIDER_EVENTS: u64 = 1_000_000;
const MAX_ID_BYTES: usize = 2 * 1024;
const MAX_LABEL_BYTES: usize = 2 * 1024;
const MAX_ALIAS_BYTES: usize = 512;
const MAX_PEERS: usize = 64;

#[derive(Clone, Debug)]
pub struct ProviderEventQuery {
    pub page: u32,
    pub size: u32,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEvent {
    pub id: String,
    pub alias: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub identity_ids: Vec<String>,
    pub source_label: Option<String>,
    pub status: Option<String>,
    pub camera_id: Option<String>,
    pub accepted: bool,
    pub ignored: bool,
    pub missed: bool,
    pub rejected: bool,
    pub unlocked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEventPage {
    pub events: Vec<ProviderEvent>,
    pub last: bool,
}

#[async_trait]
pub trait EventProvider: Send + Sync {
    async fn page(&self, query: ProviderEventQuery) -> Result<ProviderEventPage, AppError>;
}

#[derive(Clone)]
pub struct MtsEventProvider {
    client: MtsClient,
}

impl MtsEventProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EventProvider for MtsEventProvider {
    async fn page(&self, query: ProviderEventQuery) -> Result<ProviderEventPage, AppError> {
        validate_query(&query)?;
        let path = event_path(&query);
        let response: EventsDto = self
            .client
            .send_json(
                self.client
                    .authorized(Method::GET, ProviderBase::Gateway, &path)?,
            )
            .await?;
        response.into_page(&query)
    }
}

fn validate_query(query: &ProviderEventQuery) -> Result<(), AppError> {
    if query.size == 0
        || query.size > MAX_PROVIDER_PAGE_SIZE
        || query.page >= MAX_PROVIDER_PAGES
        || query.from > query.to
    {
        return Err(AppError::invalid_request("invalid provider event query"));
    }
    Ok(())
}

fn event_path(query: &ProviderEventQuery) -> String {
    let mut values = form_urlencoded::Serializer::new(String::new());
    values.append_pair("page", &query.page.to_string());
    values.append_pair("size", &query.size.to_string());
    values.append_pair("from", &provider_time(query.from));
    values.append_pair("to", &provider_time(query.to));
    format!("{EVENTS_PATH}?{}", values.finish())
}

fn provider_time(value: DateTime<Utc>) -> String {
    value
        .to_rfc3339_opts(SecondsFormat::Millis, true)
        .trim_end_matches('Z')
        .to_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsDto {
    content: Vec<EventDto>,
    first: bool,
    last: bool,
    empty: bool,
    number: u32,
    number_of_elements: usize,
    size: u32,
    total_elements: u64,
    total_pages: u32,
}

impl EventsDto {
    fn into_page(self, query: &ProviderEventQuery) -> Result<ProviderEventPage, AppError> {
        if self.number != query.page
            || self.size != query.size
            || self.content.len() > query.size as usize
            || self.number_of_elements != self.content.len()
            || self.empty != self.content.is_empty()
            || self.first != (self.number == 0)
            || self.total_pages > MAX_PROVIDER_PAGES
            || self.total_elements > MAX_PROVIDER_EVENTS
            || self.total_elements < self.content.len() as u64
            || (self.total_pages == 0 && (!self.last || !self.content.is_empty()))
            || (self.total_pages > 0 && self.number >= self.total_pages)
            || self.last != (self.total_pages == 0 || self.number + 1 == self.total_pages)
        {
            return Err(schema_error());
        }
        let events = self
            .content
            .into_iter()
            .map(ProviderEvent::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProviderEventPage {
            events,
            last: self.last,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventDto {
    id: Option<String>,
    alias: Option<String>,
    created: Option<String>,
    consumer_id: Option<String>,
    initiator_id: Option<String>,
    payload: Option<PayloadDto>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PayloadDto {
    apartment: Option<IdentityDto>,
    camera: Option<IdentityDto>,
    context: Option<IdentityDto>,
    device: Option<IdentityDto>,
    intercom: Option<IdentityDto>,
    created_at: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    status: Option<serde_json::Value>,
    peers: Option<Vec<PeerDto>>,
    #[serde(default, rename = "isAccepted")]
    accepted: bool,
    #[serde(default, rename = "isIgnored")]
    ignored: bool,
    #[serde(default, rename = "isMissed")]
    missed: bool,
    #[serde(default, rename = "isRejected")]
    rejected: bool,
    #[serde(default, rename = "isUnlocked")]
    unlocked: bool,
}

#[derive(Deserialize)]
struct IdentityDto {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerDto {
    dial_status: Option<DialStatusDto>,
    opened: Option<bool>,
}

#[derive(Deserialize)]
struct DialStatusDto {
    name: Option<String>,
}

impl TryFrom<EventDto> for ProviderEvent {
    type Error = AppError;

    fn try_from(dto: EventDto) -> Result<Self, Self::Error> {
        let id = required_value(dto.id, MAX_ID_BYTES)?;
        let alias = optional_value(dto.alias, MAX_ALIAS_BYTES)?;
        let payload = dto.payload.unwrap_or_default();
        if payload
            .peers
            .as_ref()
            .is_some_and(|peers| peers.len() > MAX_PEERS)
        {
            return Err(schema_error());
        }
        let occurred_at = dto
            .created
            .as_deref()
            .or(payload.created_at.as_deref())
            .and_then(parse_provider_time)
            .ok_or_else(schema_error)?;
        let started_at = optional_time(
            payload
                .started_at
                .as_deref()
                .or(payload.created_at.as_deref()),
        )?;
        let finished_at = optional_time(payload.finished_at.as_deref())?;
        let camera_id = payload
            .camera
            .as_ref()
            .and_then(|identity| identity.id.clone())
            .map(|value| required_value(Some(value), MAX_ID_BYTES))
            .transpose()?;
        let mut identities = BTreeSet::new();
        for candidate in [
            dto.consumer_id.as_ref(),
            dto.initiator_id.as_ref(),
            payload
                .apartment
                .as_ref()
                .and_then(|value| value.id.as_ref()),
            payload.camera.as_ref().and_then(|value| value.id.as_ref()),
            payload.context.as_ref().and_then(|value| value.id.as_ref()),
            payload.device.as_ref().and_then(|value| value.id.as_ref()),
            payload
                .intercom
                .as_ref()
                .and_then(|value| value.id.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            identities.insert(required_value(Some(candidate.clone()), MAX_ID_BYTES)?);
        }
        let source_label = [
            payload
                .intercom
                .as_ref()
                .and_then(|value| value.name.clone()),
            payload.device.as_ref().and_then(|value| value.name.clone()),
            payload.camera.as_ref().and_then(|value| value.name.clone()),
        ]
        .into_iter()
        .flatten()
        .next()
        .map(|value| required_value(Some(value), MAX_LABEL_BYTES))
        .transpose()?;
        let peer_status = payload
            .peers
            .as_ref()
            .and_then(|peers| peers.first())
            .and_then(|peer| peer.dial_status.as_ref())
            .and_then(|status| status.name.clone());
        let status = payload
            .status
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or(peer_status)
            .map(|value| required_value(Some(value.to_lowercase()), MAX_ALIAS_BYTES))
            .transpose()?;
        let peer_opened = payload
            .peers
            .as_ref()
            .is_some_and(|peers| peers.iter().any(|peer| peer.opened == Some(true)));

        Ok(Self {
            id,
            alias,
            occurred_at,
            started_at,
            finished_at,
            identity_ids: identities.into_iter().collect(),
            source_label,
            status,
            camera_id,
            accepted: payload.accepted,
            ignored: payload.ignored,
            missed: payload.missed,
            rejected: payload.rejected,
            unlocked: payload.unlocked || peer_opened,
        })
    }
}

fn required_value(value: Option<String>, limit: usize) -> Result<String, AppError> {
    let value = value.ok_or_else(schema_error)?;
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(schema_error());
    }
    Ok(value)
}

fn optional_value(value: Option<String>, limit: usize) -> Result<Option<String>, AppError> {
    value
        .map(|value| required_value(Some(value), limit))
        .transpose()
}

fn optional_time(value: Option<&str>) -> Result<Option<DateTime<Utc>>, AppError> {
    value
        .map(|value| parse_provider_time(value).ok_or_else(schema_error))
        .transpose()
}

fn parse_provider_time(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|value| value.and_utc())
                .ok()
        })
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider event response was invalid.",
        false,
    )
}
