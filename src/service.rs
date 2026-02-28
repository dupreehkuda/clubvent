use crate::err::CustomError as Err;
use crate::insights;
use crate::insights::InsightsClient;
use crate::models::*;
use crate::repository::{new_postgres_repository, Postgres, Repository};
use chrono::prelude::*;
use rand::seq::SliceRandom;
use std::env;
use std::error::Error;

pub struct Service {
    repository: Postgres,
    insights: InsightsClient,
}

impl Service {
    pub async fn register_new_club(&self, chat_id: i64) -> Result<(), Box<dyn Error>> {
        self.repository
            .register_new_club(NewClubRequest { chat_id })
            .await
            .map_err(|err| Box::new(err) as Box<dyn Error>)
    }

    pub async fn new_club_event(&self, chat_id: i64, date: &str) -> Result<String, Box<dyn Error>> {
        let dt = Utc.datetime_from_str(date, "%Y.%m.%d %H:%M");
        match dt {
            Ok(_) => {}
            Err(_) => return Err(Box::new(Err::WrongDateFormat)),
        }

        if dt.unwrap().le(&Utc::now()) {
            return Err(Box::new(Err::EventInPast));
        }

        let event_date = NaiveDateTime::from_timestamp_opt(dt.unwrap().timestamp(), 0).unwrap();

        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if !latest_event.event_id.is_nil() {
            return Err(Box::new(Err::ActiveEventFound(
                latest_event.event_date.to_string(),
            )));
        }

        let event_id = uuid::Uuid::new_v4();

        let resp = self
            .repository
            .write_new_event(NewEventRequest {
                chat_id,
                event_id,
                event_date,
            })
            .await;

        resp.unwrap();
        Ok(beautify_date(event_date))
    }

    pub async fn new_member_suggestion(
        &self,
        chat_id: i64,
        user_id: u32,
        suggestion: &str,
    ) -> Result<(), Box<dyn Error>> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if latest_event.event_id.is_nil() {
            return Err(Box::new(Err::NoActiveEventFound));
        }

        if !latest_event.subject.is_empty() {
            return Err(Box::new(Err::AlreadyPickedSubject(latest_event.subject)));
        }

        self.repository
            .write_new_member_suggestion(NewMemberSuggestion {
                event_id: latest_event.event_id,
                chat_id,
                user_id,
                suggestion: escape_markdown_v2(suggestion),
            })
            .await
            .unwrap();

        Ok(())
    }

    pub async fn toggle_with_insights(&self, chat_id: i64) -> Result<String, Box<dyn Error>> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if latest_event.event_id.is_nil() {
            return Err(Box::new(Err::NoActiveEventFound));
        }

        if !latest_event.subject.is_empty() {
            return Ok("Unable to toggle insights because subject is already picked".to_string());
        }

        self.repository
            .toggle_with_insights(EventToggleWithInsightsRequest {
                event_id: latest_event.event_id,
                with_insights: latest_event.with_insights,
            })
            .await?;

        if latest_event.with_insights {
            return Ok("Turned off insights for current event".to_string());
        } else if !latest_event.with_insights {
            return Ok("Turned on insights for current event".to_string());
        }

        Ok("Unable to toggle insights".to_string())
    }

    // start_active_event needed only to stop accepting new insights and get summary link
    pub async fn start_active_event(&self, chat_id: i64) -> Result<String, Box<dyn Error>> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if latest_event.event_id.is_nil() {
            return Err(Box::new(Err::NoActiveEventFound));
        }

        if !latest_event.with_insights {
            return Err(Box::new(Err::EventWithoutInsights));
        }

        let summary_link = self
            .insights
            .start_event(latest_event.event_id)
            .await
            .unwrap();

        Ok(format!(
            "Here is your [insights summary]({})\\.\nHave a great club\\!",
            summary_link,
        ))
    }

    pub async fn achieve_active_event(&self, chat_id: i64) -> Result<String, Box<dyn Error>> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if latest_event.event_id.is_nil() {
            return Err(Box::new(Err::NoActiveEventFound));
        }

        self.repository
            .achieve_event(AchieveEventRequest {
                chat_id,
                event_id: latest_event.event_id,
            })
            .await
            .unwrap();

        if latest_event.with_insights && !latest_event.subject.is_empty() {
            self.insights
                .finish_event(latest_event.event_id)
                .await
                .unwrap();
        }

        let formatted_date = beautify_date(latest_event.event_date);

        Ok(formatted_date)
    }

    pub async fn pick_from_suggestions(&self, chat_id: i64) -> Result<String, Box<dyn Error>> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if latest_event.event_id.is_nil() {
            return Err(Box::new(Err::NoActiveEventFound));
        }

        if !latest_event.subject.is_empty() {
            return Err(Box::new(Err::AlreadyPickedSubject(latest_event.subject)));
        }

        let suggestions = self
            .repository
            .get_all_suggestions_for_event(EventSuggestionsRequest {
                event_id: latest_event.event_id,
            })
            .await
            .unwrap()
            .suggestions;

        if suggestions.is_empty() {
            return Err(Box::new(Err::NoSuggestionsFound));
        }

        let result = suggestions.choose(&mut rand::thread_rng());

        if !latest_event.with_insights {
            self.repository
                .write_picked_subject(PickedSubjectRequest {
                    event_id: latest_event.event_id,
                    subject: result.unwrap().to_string(),
                    insights_link: None,
                })
                .await
                .unwrap();

            return Ok(format!("Randomly picked\n{}", result.unwrap()));
        }

        let insights_link = self
            .insights
            .register_event(RegisterEventRequest {
                event_id: latest_event.event_id,
                event_subject: unescape_markdown_v2(result.unwrap()),
                club_id: chat_id,
            })
            .await
            .unwrap();

        self.repository
            .write_picked_subject(PickedSubjectRequest {
                event_id: latest_event.event_id,
                subject: result.unwrap().to_string(),
                insights_link: Some(insights_link.clone()),
            })
            .await
            .unwrap();

        Ok(format!(
            "Randomly picked\n{}\n\nAnd here is your [insights link]({})",
            result.unwrap(),
            insights_link,
        ))
    }

    pub async fn get_current_event_info(&self, chat_id: i64) -> Result<String, Box<dyn Error>> {
        let latest_event = self
            .repository
            .get_latest_event(LastEventRequest { chat_id })
            .await
            .unwrap();

        if latest_event.event_id.is_nil() {
            return Err(Box::new(Err::NoActiveEventFound));
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
            formatted_date, latest_event.subject
        );

        if latest_event.with_insights {
            message = format!(
                "{}\nHere is the [insights link]({})",
                message,
                latest_event.insights_link.unwrap()
            )
        }

        Ok(message)
    }
}

pub async fn default_service() -> Service {
    let dsn = env::var("DB_DSN").unwrap();
    let repo = new_postgres_repository(dsn.as_str()).await;

    let address = env::var("INSIGHTS_ADDRESS").unwrap();
    let insights = insights::new(address);

    Service {
        repository: repo.unwrap(),
        insights,
    }
}

fn escape_markdown_v2(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        match ch {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
            | '|' | '{' | '}' | '.' | '!' | '\\' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

fn unescape_markdown_v2(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek() {
                match next {
                    '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-'
                    | '=' | '|' | '{' | '}' | '.' | '!' | '\\' => {
                        result.push(chars.next().unwrap());
                        continue;
                    }
                    _ => {}
                }
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
