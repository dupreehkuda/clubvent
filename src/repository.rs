use crate::models::*;
use async_trait::async_trait;
use bb8_postgres::bb8::Pool;
use bb8_postgres::{tokio_postgres::NoTls, PostgresConnectionManager};
use chrono::{DateTime, NaiveDateTime, Utc};
use tokio_postgres::Error;
use uuid::Uuid;

// todo use this trait in service, study box
#[async_trait]
pub trait Repository {
    async fn register_new_club(&self, req: NewClubRequest) -> Result<(), Error>;
    async fn write_new_event(&self, req: NewEventRequest) -> Result<(), Error>;
    async fn get_latest_event(&self, req: LastEventRequest) -> Result<LastEventResponse, Error>;
    async fn write_new_member_suggestion(&self, req: NewMemberSuggestion) -> Result<(), Error>;
    async fn achieve_event(&self, req: AchieveEventRequest) -> Result<(), Error>;
    async fn get_all_suggestions_for_event(
        &self,
        req: EventSuggestionsRequest,
    ) -> Result<EventSuggestionsResponse, Error>;
    async fn write_picked_subject(&self, req: PickedSubjectRequest) -> Result<(), Error>;
}

pub struct Postgres {
    pool: Pool<PostgresConnectionManager<NoTls>>,
}

pub async fn new_postgres_repository(dsn: &str) -> Result<Postgres, Error> {
    let manager = PostgresConnectionManager::new(dsn.parse()?, NoTls);
    let pool = Pool::builder().build(manager).await.unwrap();

    Ok(Postgres { pool })
}

#[async_trait]
impl Repository for Postgres {
    async fn register_new_club(&self, req: NewClubRequest) -> Result<(), Error> {
        let conn = self.pool.get().await.unwrap();
        let result = conn
            .execute("INSERT INTO club (chat_id) VALUES ($1);", &[&req.chat_id])
            .await;

        result.map(|_| ())
    }

    async fn write_new_event(&self, req: NewEventRequest) -> Result<(), Error> {
        let mut conn = self.pool.get().await.unwrap();
        let tx = conn.transaction().await.unwrap();

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

        tx.commit().await
    }

    async fn get_latest_event(&self, req: LastEventRequest) -> Result<LastEventResponse, Error> {
        let conn = self.pool.get().await.unwrap();
        let result = conn
            .query(
                "SELECT id, event_date, subject, insights, insights_link FROM events WHERE chat_id = $1 AND active = true;",
                &[&req.chat_id],
            )
            .await
            .unwrap();

        if result.is_empty() {
            return Ok(LastEventResponse {
                event_id: Uuid::default(),
                event_date: NaiveDateTime::default(),
                subject: String::new(),
                with_insights: false,
                insights_link: None,
            });
        }

        let event_id = result[0].get(0);
        let event_date: DateTime<Utc> = result[0].get(1);
        let subject: Option<String> = result[0].get(2);
        let with_insights: bool = result[0].get(3);
        let insights_link: Option<String> = result[0].get(4);

        Ok(LastEventResponse {
            event_id,
            event_date: event_date.naive_utc(),
            subject: subject.unwrap_or_default(),
            with_insights,
            insights_link,
        })
    }

    async fn write_new_member_suggestion(&self, req: NewMemberSuggestion) -> Result<(), Error> {
        let conn = self.pool.get().await.unwrap();
        let result = conn
            .execute(
            "INSERT INTO suggestions (event_id, chat_id, user_id, suggestion) VALUES ($1, $2, $3, $4);",
            &[&req.event_id, &req.chat_id, &(req.user_id as i64), &req.suggestion]
            )
            .await;

        result.map(|_| ())
    }

    async fn achieve_event(&self, req: AchieveEventRequest) -> Result<(), Error> {
        let mut conn = self.pool.get().await.unwrap();
        let tx = conn.transaction().await.unwrap();

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

        tx.commit().await
    }

    async fn get_all_suggestions_for_event(
        &self,
        req: EventSuggestionsRequest,
    ) -> Result<EventSuggestionsResponse, Error> {
        let conn = self.pool.get().await.unwrap();
        let result = conn
            .query(
                "SELECT suggestion FROM suggestions WHERE event_id = $1;",
                &[&req.event_id],
            )
            .await
            .unwrap();

        let mut ans = EventSuggestionsResponse {
            suggestions: vec![],
        };

        for row in result {
            ans.suggestions.push(row.get(0))
        }

        Ok(ans)
    }

    async fn write_picked_subject(&self, req: PickedSubjectRequest) -> Result<(), Error> {
        let conn = self.pool.get().await.unwrap();
        let result = conn
            .execute(
                "UPDATE events SET subject = $1, insights_link = $2 WHERE id = $3;",
                &[&req.subject, &req.insights_link, &req.event_id],
            )
            .await;

        result.map(|_| ())
    }

}
