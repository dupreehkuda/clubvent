//! Tests over the real `Service`: every test gets its own Postgres container
//! (testcontainers, needs Docker) with the schema applied, and insights replaced
//! by a wiremock server. Set `TEST_DB_DSN` to reuse a running Postgres instead.

use super::*;
use crate::err::CustomError;
use serde_json::json;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio_postgres::error::SqlState;
use tokio_postgres::NoTls;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SCHEMA: &str = include_str!("../scheme.sql");
const RESERVED: &str = "_*[]()~`>#+-=|{}.!\\";

// ====================================================================
// helpers
// ====================================================================

/// Fails on any reserved char that is not escaped; understands `[text](url)` links,
/// where inside the url only `)` and `\` are reserved.
fn assert_markdown_v2(s: &str) {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '[' => {
                i += 1;
                while c[i] != ']' {
                    if c[i] == '\\' {
                        i += 1;
                    } else {
                        assert!(!RESERVED.contains(c[i]), "unescaped {:?} in {:?}", c[i], s);
                    }
                    i += 1;
                }
                assert_eq!(c[i + 1], '(', "link without url in {:?}", s);
                i += 2;
                while c[i] != ')' {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            ch => {
                assert!(!RESERVED.contains(ch), "unescaped {:?} in {:?}", ch, s);
                i += 1;
            }
        }
    }
}

fn domain(err: &AppError) -> &CustomError {
    err.downcast_ref::<CustomError>()
        .unwrap_or_else(|| panic!("expected a domain error, got {}", err))
}

fn future() -> String {
    (Utc::now() + chrono::Duration::days(30))
        .format("%Y.%m.%d %H:%M")
        .to_string()
}

/// Insights replaced by a wiremock server: healthy by default, `set_failing` turns every call into a 500.
struct Stub {
    server: MockServer,
}

impl Stub {
    async fn start() -> Stub {
        let stub = Stub {
            server: MockServer::start().await,
        };
        stub.set_failing(false).await;
        stub
    }

    fn url(&self) -> String {
        self.server.uri()
    }

    async fn set_failing(&self, failing: bool) {
        self.server.reset().await;
        if failing {
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&self.server)
                .await;
            return;
        }
        Mock::given(method("POST"))
            .and(path("/api/v1/event/register"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "insights_link": "http://stub/insights/1" })),
            )
            .mount(&self.server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/event/start"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({ "summary_link": "http://stub/summary/1", "error": null }),
                ),
            )
            .mount(&self.server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/event/finish"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&self.server)
            .await;
    }

    async fn last_body(&self) -> serde_json::Value {
        let requests = self.server.received_requests().await.unwrap();
        requests
            .last()
            .expect("insights was called")
            .body_json()
            .unwrap()
    }
}

/// One isolated club: its own Postgres, stub insights, fresh chat id.
struct Env {
    service: Service,
    stub: Stub,
    db: tokio_postgres::Client,
    dsn: String,
    chat: i64,
    // dropped with the test, which removes the container
    _pg: Option<ContainerAsync<Postgres>>,
}

/// testcontainers only reads DOCKER_HOST while the docker CLI also honours contexts; follow the CLI.
fn follow_docker_context() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("DOCKER_HOST").is_some() {
            return;
        }
        let out = std::process::Command::new("docker")
            .args([
                "context",
                "inspect",
                "--format",
                "{{.Endpoints.docker.Host}}",
            ])
            .output();
        if let Ok(out) = out {
            let host = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !host.is_empty() {
                std::env::set_var("DOCKER_HOST", host);
            }
        }
    });
}

async fn setup() -> Env {
    let (_pg, dsn) = match std::env::var("TEST_DB_DSN") {
        Ok(dsn) => (None, dsn),
        Err(_) => {
            follow_docker_context();
            let pg = Postgres::default()
                .with_tag("16-alpine")
                .start()
                .await
                .expect("start postgres container: is docker running?");
            let port = pg.get_host_port_ipv4(5432).await.unwrap();
            let dsn =
                format!("postgresql://127.0.0.1:{port}/postgres?user=postgres&password=postgres");
            (Some(pg), dsn)
        }
    };

    let (db, conn) = tokio_postgres::connect(&dsn, NoTls)
        .await
        .expect("connect test db");
    tokio::spawn(conn);
    // advisory lock: with TEST_DB_DSN parallel tests share one database
    db.batch_execute(&format!(
        "BEGIN; SELECT pg_advisory_xact_lock(4242); {} COMMIT;",
        SCHEMA
    ))
    .await
    .expect("apply schema");

    let stub = Stub::start().await;
    let service = Service {
        repository: new_postgres_repository(&dsn).await.unwrap(),
        insights: insights::new(stub.url()),
    };
    // negative like telegram group ids, unique per test
    let chat = -(rand::random::<u32>() as i64) - 1;
    Env {
        service,
        stub,
        db,
        dsn,
        chat,
        _pg,
    }
}

impl Env {
    async fn club(&self) {
        self.service.register_new_club(self.chat).await.unwrap();
    }

    async fn club_with_event(&self) {
        self.club().await;
        self.service
            .new_club_event(self.chat, &future())
            .await
            .unwrap();
    }

    async fn club_with_suggestion(&self, text: &str) {
        self.club_with_event().await;
        self.service
            .new_member_suggestion(self.chat, 1, text)
            .await
            .unwrap();
    }

    async fn club_with_picked(&self, text: &str) {
        self.club_with_suggestion(text).await;
        self.service.pick_from_suggestions(self.chat).await.unwrap();
    }

    /// Row as the 2024 code wrote it: MarkdownV2-escaped, hyphen only.
    async fn insert_old_suggestion(&self, escaped: &str) {
        self.db
            .execute(
                "INSERT INTO suggestions (event_id, chat_id, user_id, suggestion) \
                 SELECT id, chat_id, 1, $2 FROM events WHERE chat_id = $1 AND active",
                &[&self.chat, &escaped],
            )
            .await
            .unwrap();
    }

    async fn set_old_subject(&self, escaped: &str) {
        self.db
            .execute(
                "UPDATE events SET subject = $2 WHERE chat_id = $1 AND active",
                &[&self.chat, &escaped],
            )
            .await
            .unwrap();
    }

    async fn stored_subject(&self) -> Option<String> {
        self.db
            .query_one(
                "SELECT subject FROM events WHERE chat_id = $1 AND active",
                &[&self.chat],
            )
            .await
            .unwrap()
            .get(0)
    }

    async fn stored_suggestion(&self) -> (i64, String) {
        let row = self
            .db
            .query_one(
                "SELECT user_id, suggestion FROM suggestions WHERE chat_id = $1",
                &[&self.chat],
            )
            .await
            .unwrap();
        (row.get(0), row.get(1))
    }
}

// ====================================================================
// escaping (no database)
// ====================================================================

#[test]
fn every_kind_of_dash_survives_escaping() {
    // Arrange.
    let inputs = [
        "Здесь была Бритт-Мари, Фредрик Бакман",
        "Мастер и Маргарита — Булгаков",
        "Сто лет одиночества – Маркес",
        "a - b – c — d ‒ e − f",
        "--двойной-- и ---тройной---",
        "-в начале и в конце-",
        "с точкой. и скобками (x) и !",
        "\\уже с бэкслешем\\-",
    ];

    for input in inputs {
        // Act.
        let escaped = escape_markdown_v2(input);

        // Assert.
        assert_markdown_v2(&escaped);
        assert_eq!(
            unescape_markdown_v2(&escaped),
            input,
            "roundtrip {:?}",
            input
        );
    }
}

#[test]
fn text_stored_by_old_code_is_normalized() {
    // Arrange.
    let old_row = "Ф. Бакман \\- Бритт\\-Мари";
    let raw_row = "Просто текст, без магии";

    // Act.
    let from_old = stored_text(old_row);
    let from_raw = stored_text(raw_row);

    // Assert.
    assert_eq!(from_old, "Ф. Бакман - Бритт-Мари");
    assert_eq!(from_raw, raw_row);
    assert_markdown_v2(&escape_markdown_v2(&from_old));
}

#[test]
fn link_url_escapes_only_parenthesis_and_backslash() {
    // Arrange.
    let url = "https://x.y/a-b_c.(d)";

    // Act.
    let escaped = escape_link_url(url);

    // Assert.
    assert_eq!(escaped, "https://x.y/a-b_c.(d\\)");
    assert_markdown_v2(&format!("[x]({})", escaped));
}

// ====================================================================
// club and event lifecycle
// ====================================================================

#[tokio::test(flavor = "multi_thread")]
async fn club_can_be_registered_only_once() {
    // Arrange.
    let t = setup().await;
    t.club().await;

    // Act.
    let err = t.service.register_new_club(t.chat).await.unwrap_err();

    // Assert.
    let code = err
        .downcast_ref::<tokio_postgres::Error>()
        .and_then(|e| e.code());
    assert_eq!(code, Some(&SqlState::UNIQUE_VIOLATION));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_command_needs_an_active_event() {
    // Arrange.
    let t = setup().await;
    t.club().await;

    // Act.
    let errors = [
        t.service
            .new_member_suggestion(t.chat, 1, "x")
            .await
            .unwrap_err(),
        t.service.pick_from_suggestions(t.chat).await.unwrap_err(),
        t.service.get_current_event_info(t.chat).await.unwrap_err(),
        t.service.start_active_event(t.chat).await.unwrap_err(),
        t.service.achieve_active_event(t.chat).await.unwrap_err(),
    ];

    // Assert.
    for err in &errors {
        assert!(matches!(domain(err), CustomError::NoActiveEventFound));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn event_rejects_wrong_date_format() {
    // Arrange.
    let t = setup().await;
    t.club().await;

    // Act.
    let err = t
        .service
        .new_club_event(t.chat, "2030-01-01 15:00")
        .await
        .unwrap_err();

    // Assert.
    assert!(matches!(domain(&err), CustomError::WrongDateFormat));
}

#[tokio::test(flavor = "multi_thread")]
async fn event_rejects_a_date_in_the_past() {
    // Arrange.
    let t = setup().await;
    t.club().await;

    // Act.
    let err = t
        .service
        .new_club_event(t.chat, "2020.01.01 15:00")
        .await
        .unwrap_err();

    // Assert.
    assert!(matches!(domain(&err), CustomError::EventInPast));
}

#[tokio::test(flavor = "multi_thread")]
async fn event_is_created_with_a_readable_date() {
    // Arrange.
    let t = setup().await;
    t.club().await;

    // Act.
    let text = t.service.new_club_event(t.chat, &future()).await.unwrap();

    // Assert.
    assert!(text.contains(" of "), "beautified date, got {:?}", text);
}

#[tokio::test(flavor = "multi_thread")]
async fn second_event_is_refused_while_one_is_active() {
    // Arrange.
    let t = setup().await;
    t.club_with_event().await;

    // Act.
    let err = t
        .service
        .new_club_event(t.chat, &future())
        .await
        .unwrap_err();

    // Assert.
    assert!(matches!(domain(&err), CustomError::ActiveEventFound(d) if d.contains(" of ")));
}

#[tokio::test(flavor = "multi_thread")]
async fn current_before_pick_says_subject_is_not_picked() {
    // Arrange.
    let t = setup().await;
    t.club_with_event().await;

    // Act.
    let info = t.service.get_current_event_info(t.chat).await.unwrap();

    // Assert.
    assert_markdown_v2(&info);
    assert!(info.contains("hasn't been picked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pick_without_suggestions_is_refused() {
    // Arrange.
    let t = setup().await;
    t.club_with_event().await;

    // Act.
    let err = t.service.pick_from_suggestions(t.chat).await.unwrap_err();

    // Assert.
    assert!(matches!(domain(&err), CustomError::NoSuggestionsFound));
}

// ====================================================================
// suggestions and picking
// ====================================================================

#[tokio::test(flavor = "multi_thread")]
async fn suggestion_is_stored_raw_with_full_user_id() {
    // Arrange.
    let t = setup().await;
    t.club_with_event().await;
    let raw = "Ф. Бакман — «Здесь была Бритт-Мари» (2014)!";
    let big_user_id = 7_000_000_000;

    // Act.
    t.service
        .new_member_suggestion(t.chat, big_user_id, raw)
        .await
        .unwrap();

    // Assert.
    let (user_id, suggestion) = t.stored_suggestion().await;
    assert_eq!(suggestion, raw, "no escaping in the database");
    assert_eq!(
        user_id, big_user_id as i64,
        "telegram ids above u32 survive"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pick_renders_reserved_chars_and_sends_raw_subject_to_insights() {
    // Arrange.
    let t = setup().await;
    let raw = "Ф. Бакман — «Здесь была Бритт-Мари» (2014)!";
    t.club_with_suggestion(raw).await;

    // Act.
    let msg = t.service.pick_from_suggestions(t.chat).await.unwrap();

    // Assert.
    assert_markdown_v2(&msg);
    assert!(msg.contains(&escape_markdown_v2(raw)), "{:?}", msg);
    assert!(msg.contains("(http://stub/insights/1)"), "{:?}", msg);
    assert_eq!(
        t.stub.last_body().await["event_subject"],
        raw,
        "insights gets raw text"
    );
    assert_eq!(t.stored_subject().await.as_deref(), Some(raw));
}

#[tokio::test(flavor = "multi_thread")]
async fn picked_event_refuses_another_pick_and_late_suggestions() {
    // Arrange.
    let t = setup().await;
    let raw = "Ф. Бакман — «Здесь была Бритт-Мари» (2014)!";
    t.club_with_picked(raw).await;

    // Act.
    let pick_err = t.service.pick_from_suggestions(t.chat).await.unwrap_err();
    let suggest_err = t
        .service
        .new_member_suggestion(t.chat, 1, "late")
        .await
        .unwrap_err();

    // Assert.
    assert!(matches!(domain(&pick_err), CustomError::AlreadyPickedSubject(s) if s == raw));
    assert!(matches!(domain(&suggest_err), CustomError::AlreadyPickedSubject(s) if s == raw));
}

#[tokio::test(flavor = "multi_thread")]
async fn current_shows_picked_subject_and_insights_link() {
    // Arrange.
    let t = setup().await;
    let raw = "Ф. Бакман — «Здесь была Бритт-Мари» (2014)!";
    t.club_with_picked(raw).await;

    // Act.
    let info = t.service.get_current_event_info(t.chat).await.unwrap();

    // Assert.
    assert_markdown_v2(&info);
    assert!(info.contains(&escape_markdown_v2(raw)), "{:?}", info);
    assert!(info.contains("[insights link](http://stub/insights/1)"));
}

// ====================================================================
// rows written by the 2024 code (escaped in the database)
// ====================================================================

#[tokio::test(flavor = "multi_thread")]
async fn old_suggestion_is_picked_and_normalized() {
    // Arrange.
    let t = setup().await;
    t.club_with_event().await;
    t.insert_old_suggestion("Ф. Бакман \\- Бритт\\-Мари").await;

    // Act.
    let msg = t.service.pick_from_suggestions(t.chat).await.unwrap();

    // Assert.
    assert_markdown_v2(&msg);
    assert!(msg.contains("Ф\\. Бакман \\- Бритт\\-Мари"), "{:?}", msg);
    assert_eq!(
        t.stored_subject().await.as_deref(),
        Some("Ф. Бакман - Бритт-Мари")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn old_subject_still_renders_in_current_and_pick() {
    // Arrange.
    let t = setup().await;
    t.club_with_event().await;
    t.set_old_subject("Кафка. Процесс \\- 1925").await;

    // Act.
    let info = t.service.get_current_event_info(t.chat).await.unwrap();
    let err = t.service.pick_from_suggestions(t.chat).await.unwrap_err();

    // Assert.
    assert_markdown_v2(&info);
    assert!(info.contains("Кафка\\. Процесс \\- 1925"), "{:?}", info);
    assert!(
        matches!(domain(&err), CustomError::AlreadyPickedSubject(s) if s == "Кафка. Процесс - 1925")
    );
}

// ====================================================================
// insights failures
// ====================================================================

#[tokio::test(flavor = "multi_thread")]
async fn failing_insights_makes_pick_an_error_and_writes_nothing() {
    // Arrange.
    let t = setup().await;
    t.club_with_suggestion("Процесс").await;
    t.stub.set_failing(true).await;

    // Act.
    let err = t.service.pick_from_suggestions(t.chat).await.unwrap_err();

    // Assert.
    assert!(
        err.downcast_ref::<CustomError>().is_none(),
        "infra error, not a domain one"
    );
    assert_eq!(t.stored_subject().await, None, "pick can be retried");
}

#[tokio::test(flavor = "multi_thread")]
async fn pick_succeeds_once_insights_recovers() {
    // Arrange.
    let t = setup().await;
    t.club_with_suggestion("Процесс").await;
    t.stub.set_failing(true).await;
    t.service.pick_from_suggestions(t.chat).await.unwrap_err();
    t.stub.set_failing(false).await;

    // Act.
    let msg = t.service.pick_from_suggestions(t.chat).await.unwrap();

    // Assert.
    assert_markdown_v2(&msg);
    assert_eq!(t.stored_subject().await.as_deref(), Some("Процесс"));
}

#[tokio::test(flavor = "multi_thread")]
async fn start_returns_summary_link() {
    // Arrange.
    let t = setup().await;
    t.club_with_picked("Процесс").await;

    // Act.
    let summary = t.service.start_active_event(t.chat).await.unwrap();

    // Assert.
    assert_markdown_v2(&summary);
    assert!(summary.contains("(http://stub/summary/1)"), "{:?}", summary);
}

#[tokio::test(flavor = "multi_thread")]
async fn start_fails_when_insights_fails() {
    // Arrange.
    let t = setup().await;
    t.club_with_picked("Процесс").await;
    t.stub.set_failing(true).await;

    // Act.
    let result = t.service.start_active_event(t.chat).await;

    // Assert.
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn achieve_archives_even_when_insights_fails() {
    // Arrange.
    let t = setup().await;
    t.club_with_picked("Процесс").await;
    t.stub.set_failing(true).await;

    // Act.
    let date = t.service.achieve_active_event(t.chat).await.unwrap();

    // Assert.
    assert!(date.contains(" of "), "{:?}", date);
    let err = t.service.get_current_event_info(t.chat).await.unwrap_err();
    assert!(matches!(domain(&err), CustomError::NoActiveEventFound));
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_insights_is_an_error_not_a_panic() {
    // Arrange.
    let t = setup().await;
    let service = Service {
        repository: new_postgres_repository(&t.dsn).await.unwrap(),
        insights: insights::new("http://127.0.0.1:1".to_string()),
    };
    service.register_new_club(t.chat).await.unwrap();
    service.new_club_event(t.chat, &future()).await.unwrap();
    service.new_member_suggestion(t.chat, 1, "x").await.unwrap();

    // Act.
    let err = service.pick_from_suggestions(t.chat).await.unwrap_err();

    // Assert.
    assert!(err.downcast_ref::<CustomError>().is_none());
    assert_eq!(t.stored_subject().await, None);
}
