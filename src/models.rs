use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub struct NewClubRequest {
    pub chat_id: i64,
}

pub struct NewEventRequest {
    pub chat_id: i64,
    pub event_id: Uuid,
    pub event_date: NaiveDateTime,
}

pub struct LastEventRequest {
    pub chat_id: i64,
}

pub struct AchieveEventRequest {
    pub event_id: Uuid,
    pub chat_id: i64,
}

pub struct LastEventResponse {
    pub event_id: Uuid,
    pub event_date: NaiveDateTime,
    pub subject: String,
    pub with_insights: bool,
    pub insights_link: Option<String>,
}

pub struct NewMemberSuggestion {
    pub event_id: Uuid,
    pub chat_id: i64,
    pub user_id: u32,
    pub suggestion: String,
}

pub struct EventSuggestionsRequest {
    pub event_id: Uuid,
}


pub struct EventSuggestionsResponse {
    pub suggestions: Vec<String>,
}

pub struct PickedSubjectRequest {
    pub event_id: Uuid,
    pub subject: String,
    pub insights_link: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct RegisterEventRequest {
    pub event_id: Uuid,
    pub event_subject: String,
    pub club_id: i64,
}

#[derive(Deserialize, Serialize)]
pub struct RegisterEventResponse {
    pub insights_link: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ManageEventRequest {
    pub event_id: Uuid,
}

#[derive(Deserialize, Serialize)]
pub struct StartEventResponse {
    pub summary_link: String,
    pub error: Option<String>,
}
