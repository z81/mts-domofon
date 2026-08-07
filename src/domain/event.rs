use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::FlatId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Call,
    Access,
    Camera,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Accepted,
    Missed,
    Rejected,
    Ignored,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventItem {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub kind: EventKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_outcome: Option<CallOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub items: Vec<EventItem>,
    pub page: u32,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventQuery {
    pub flat_id: FlatId,
    pub page: u32,
    pub size: u32,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub kind: Option<EventKind>,
}
