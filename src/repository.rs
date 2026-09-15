use crate::err::AppError;
use crate::models::*;
use async_trait::async_trait;
use bb8_postgres::bb8::Pool;
use bb8_postgres::{tokio_postgres::NoTls, PostgresConnectionManager};
use chrono::{DateTime, NaiveDateTime, Utc};
use uuid::Uuid;

#[async_trait]
pub trait Repository {
    async fn register_new_club(&self, req: NewClubRequest) -> Result<(), AppError>;
    async fn write_new_event(&self, req: NewEventRequest) -> Result<(), AppError>;
    async fn get_latest_event(&self, req: LastEventRequest) -> Result<LastEventResponse, AppError>;
    async fn write_new_member_suggestion(&self, req: NewMemberSuggestion) -> Result<(), AppError>;
    async fn achieve_event(&self, req: AchieveEventRequest) -> Result<(), AppError>;
    async fn get_all_suggestions_for_event(
        &self,
        req: EventSuggestionsRequest,
    ) -> Result<EventSuggestionsResponse, AppError>;
    async fn write_picked_subject(&self, req: PickedSubjectRequest) -> Result<(), AppError>;
}

pub struct Postgres {
    pool: Pool<PostgresConnectionManager<NoTls>>,
}

pub async fn new_postgres_repository(dsn: &str) -> Result<Postgres, AppError> {
    let manager = PostgresConnectionManager::new(dsn.parse()?, NoTls);
    let pool = Pool::builder().build(manager).await?;

    Ok(Postgres { pool })
}

#[async_trait]
impl Repository for Postgres {
    async fn register_new_club(&self, req: NewClubRequest) -> Result<(), AppError> {
        let conn = self.pool.get().await?;
        conn.execute("INSERT INTO club (chat_id) VALUES ($1);", &[&req.chat_id])
            .await?;

        Ok(())
    }

    async fn write_new_event(&self, req: NewEventRequest) -> Result<(), AppError> {
        let mut conn = self.pool.get().await?;
        let tx = conn.transaction().await?;

        tx.execute(
            "INSERT INTO events (id, chat_id, event_date, active, insights) VALUES ($1, $2, $3, true, true);",
            &[&req.event_id, &req.chat_id, &req.event_date.and_utc()],
        )
        .await?;

        tx.execute(
            "UPDATE club SET active_event = $1, next_event = $3 WHERE chat_id = $2;",
            &[&req.event_id, &req.chat_id, &req.event_date],
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn get_latest_event(&self, req: LastEventRequest) -> Result<LastEventResponse, AppError> {
        let conn = self.pool.get().await?;
        let result = conn
            .query(
                "SELECT id, event_date, subject, insights, insights_link FROM events WHERE chat_id = $1 AND active = true ORDER BY created_at DESC LIMIT 1;",
                &[&req.chat_id],
            )
            .await?;

        let Some(row) = result.first() else {
            return Ok(LastEventResponse {
                event_id: Uuid::default(),
                event_date: NaiveDateTime::default(),
                subject: String::new(),
                with_insights: false,
                insights_link: None,
            });
        };

        let event_date: DateTime<Utc> = row.get(1);
        let subject: Option<String> = row.get(2);

        Ok(LastEventResponse {
            event_id: row.get(0),
            event_date: event_date.naive_utc(),
            subject: subject.unwrap_or_default(),
            with_insights: row.get(3),
            insights_link: row.get(4),
        })
    }

    async fn write_new_member_suggestion(&self, req: NewMemberSuggestion) -> Result<(), AppError> {
        let conn = self.pool.get().await?;
        conn.execute(
            "INSERT INTO suggestions (event_id, chat_id, user_id, suggestion) VALUES ($1, $2, $3, $4);",
            &[&req.event_id, &req.chat_id, &(req.user_id as i64), &req.suggestion],
        )
        .await?;

        Ok(())
    }

    async fn achieve_event(&self, req: AchieveEventRequest) -> Result<(), AppError> {
        let mut conn = self.pool.get().await?;
        let tx = conn.transaction().await?;

        tx.execute(
            "UPDATE events SET active = false, achieved_on = now() WHERE id = $1;",
            &[&req.event_id],
        )
        .await?;

        tx.execute(
            "UPDATE club SET active_event = null, last_event = now(), next_event = null WHERE chat_id = $1;",
            &[&req.chat_id],
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn get_all_suggestions_for_event(
        &self,
        req: EventSuggestionsRequest,
    ) -> Result<EventSuggestionsResponse, AppError> {
        let conn = self.pool.get().await?;
        let rows = conn
            .query(
                "SELECT suggestion FROM suggestions WHERE event_id = $1;",
                &[&req.event_id],
            )
            .await?;

        Ok(EventSuggestionsResponse {
            suggestions: rows.iter().map(|row| row.get(0)).collect(),
        })
    }

    async fn write_picked_subject(&self, req: PickedSubjectRequest) -> Result<(), AppError> {
        let conn = self.pool.get().await?;
        conn.execute(
            "UPDATE events SET subject = $1, insights_link = $2 WHERE id = $3;",
            &[&req.subject, &req.insights_link, &req.event_id],
        )
        .await?;

        Ok(())
    }
}
