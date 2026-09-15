use crate::err::{AppError, CustomError as Err};
use crate::insights;
use crate::insights::InsightsClient;
use crate::models::*;
use crate::repository::{new_postgres_repository, Postgres, Repository};
use chrono::prelude::*;
use rand::seq::SliceRandom;
use std::env;

pub struct Service {
    repository: Postgres,
    insights: InsightsClient,
}

impl Service {
    pub async fn register_new_club(&self, chat_id: i64) -> Result<(), AppError> {
        self.repository
            .register_new_club(NewClubRequest { chat_id })
            .await
    }

    pub async fn new_club_event(&self, chat_id: i64, date: &str) -> Result<String, AppError> {
        let dt = Utc
            .datetime_from_str(date, "%Y.%m.%d %H:%M")
            .map_err(|_| Err::WrongDateFormat)?;

        if dt <= Utc::now() {
            return Err(Err::EventInPast.into());
        }

        let event_date = dt.naive_utc();

        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await?;

        if !latest_event.event_id.is_nil() {
            return Err(Err::ActiveEventFound(beautify_date(latest_event.event_date)).into());
        }

        self.repository
            .write_new_event(NewEventRequest {
                chat_id,
                event_id: uuid::Uuid::new_v4(),
                event_date,
            })
            .await?;

        Ok(beautify_date(event_date))
    }

    pub async fn new_member_suggestion(
        &self,
        chat_id: i64,
        user_id: u64,
        suggestion: &str,
    ) -> Result<(), AppError> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await?;

        if latest_event.event_id.is_nil() {
            return Err(Err::NoActiveEventFound.into());
        }

        if !latest_event.subject.is_empty() {
            return Err(Err::AlreadyPickedSubject(stored_text(&latest_event.subject)).into());
        }

        self.repository
            .write_new_member_suggestion(NewMemberSuggestion {
                event_id: latest_event.event_id,
                chat_id,
                user_id,
                suggestion: suggestion.to_string(),
            })
            .await
    }

    pub async fn toggle_with_insights(&self, _chat_id: i64) -> Result<String, AppError> {
        Ok("Insights are on by default for every event".to_string())
    }

    // start_active_event needed only to stop accepting new insights and get summary link
    pub async fn start_active_event(&self, chat_id: i64) -> Result<String, AppError> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await?;

        if latest_event.event_id.is_nil() {
            return Err(Err::NoActiveEventFound.into());
        }

        if !latest_event.with_insights {
            return Err(Err::EventWithoutInsights.into());
        }

        let summary_link = self.insights.start_event(latest_event.event_id).await?;

        Ok(format!(
            "Here is your [insights summary]({})\\.\nHave a great club\\!",
            escape_link_url(&summary_link),
        ))
    }

    pub async fn achieve_active_event(&self, chat_id: i64) -> Result<String, AppError> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await?;

        if latest_event.event_id.is_nil() {
            return Err(Err::NoActiveEventFound.into());
        }

        self.repository
            .achieve_event(AchieveEventRequest {
                chat_id,
                event_id: latest_event.event_id,
            })
            .await?;

        // the event is archived locally already; a failing insights call must not undo that
        if latest_event.with_insights && !latest_event.subject.is_empty() {
            if let Err(err) = self.insights.finish_event(latest_event.event_id).await {
                log::error!("finish insights for {}: {}", latest_event.event_id, err);
            }
        }

        Ok(beautify_date(latest_event.event_date))
    }

    pub async fn pick_from_suggestions(&self, chat_id: i64) -> Result<String, AppError> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await?;

        if latest_event.event_id.is_nil() {
            return Err(Err::NoActiveEventFound.into());
        }

        if !latest_event.subject.is_empty() {
            return Err(Err::AlreadyPickedSubject(stored_text(&latest_event.subject)).into());
        }

        let suggestions = self
            .repository
            .get_all_suggestions_for_event(EventSuggestionsRequest {
                event_id: latest_event.event_id,
            })
            .await?
            .suggestions;

        let picked = match suggestions.choose(&mut rand::thread_rng()) {
            Some(s) => stored_text(s),
            None => return Err(Err::NoSuggestionsFound.into()),
        };

        if !latest_event.with_insights {
            self.repository
                .write_picked_subject(PickedSubjectRequest {
                    event_id: latest_event.event_id,
                    subject: picked.clone(),
                    insights_link: None,
                })
                .await?;

            return Ok(format!("Randomly picked\n{}", escape_markdown_v2(&picked)));
        }

        let insights_link = self
            .insights
            .register_event(RegisterEventRequest {
                event_id: latest_event.event_id,
                event_subject: picked.clone(),
                club_id: chat_id,
            })
            .await?;

        self.repository
            .write_picked_subject(PickedSubjectRequest {
                event_id: latest_event.event_id,
                subject: picked.clone(),
                insights_link: Some(insights_link.clone()),
            })
            .await?;

        Ok(format!(
            "Randomly picked\n{}\n\nAnd here is your [insights link]({})",
            escape_markdown_v2(&picked),
            escape_link_url(&insights_link),
        ))
    }

    pub async fn get_current_event_info(&self, chat_id: i64) -> Result<String, AppError> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await?;

        if latest_event.event_id.is_nil() {
            return Err(Err::NoActiveEventFound.into());
        }

        let formatted_date = beautify_date(latest_event.event_date);

        if latest_event.subject.is_empty() {
            return Ok(format!(
                "The next event is on {}\\.\nThe subject hasn't been picked yet",
                formatted_date,
            ));
        }

        let mut message = format!(
            "The next event is on {}\\.\nThe subject is \\- {}",
            formatted_date,
            escape_markdown_v2(&stored_text(&latest_event.subject))
        );

        if let Some(link) = latest_event
            .insights_link
            .filter(|_| latest_event.with_insights)
        {
            message = format!(
                "{}\nHere is the [insights link]({})",
                message,
                escape_link_url(&link)
            )
        }

        Ok(message)
    }
}

pub async fn default_service() -> Result<Service, AppError> {
    let dsn = env::var("DB_DSN")?;
    let repository = new_postgres_repository(dsn.as_str()).await?;

    let address = env::var("INSIGHTS_ADDRESS")?;
    let insights = insights::new(address);

    Ok(Service {
        repository,
        insights,
    })
}

/// Text is stored raw. Rows written before September 2026 were stored already
/// MarkdownV2-escaped, so unescape on read; for raw text this is a no-op.
// ponytail: drop once no active events from before that date are left
fn stored_text(text: &str) -> String {
    unescape_markdown_v2(text)
}

fn escape_markdown_v2(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        match ch {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|'
            | '{' | '}' | '.' | '!' | '\\' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

/// Inside the `(...)` part of an inline link only `)` and `\` are reserved.
fn escape_link_url(url: &str) -> String {
    url.replace('\\', "\\\\").replace(')', "\\)")
}

fn unescape_markdown_v2(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(
                next @ ('_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
                | '|' | '{' | '}' | '.' | '!' | '\\'),
            ) = chars.peek().copied()
            {
                result.push(next);
                chars.next();
                continue;
            }
        }
        result.push(ch);
    }
    result
}

fn beautify_date(ts: NaiveDateTime) -> String {
    let day = match ts.day() {
        1 | 21 | 31 => format!("{}st", ts.day()),
        2 | 22 => format!("{}nd", ts.day()),
        3 | 23 => format!("{}rd", ts.day()),
        _ => format!("{}th", ts.day()),
    };

    format!(
        "{}, {} of {} at {}",
        ts.format("%A"),
        day,
        ts.format("%B"),
        ts.format("%H:%M")
    )
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
