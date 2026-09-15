use crate::err::{AppError, CustomError as Err};
use crate::service::{default_service, Service};
use dotenv::dotenv;
use lazy_static::lazy_static;
use teloxide::types::ParseMode::MarkdownV2;
use teloxide::{prelude::*, types::Message, utils::command::BotCommands};
use tokio::runtime::Handle;
use tokio_postgres::error::SqlState;

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Command {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "starts club", parse_with = "split")]
    Start,
    #[command(description = "create new event")]
    Event(String),
    #[command(description = "make new suggestion")]
    Suggest(String),
    #[command(description = "achieves active event")]
    Achieve,
    #[command(description = "picks a subject for active event")]
    Pick,
    #[command(description = "current event info")]
    Current,
    #[command(description = "turns insights on/off for current event")]
    Insights,
    #[command(description = "starts current event only if insights enabled (to get summary link)")]
    StartClub,
}

fn default_service_blocking() -> Service {
    let rt = Handle::current();
    tokio::task::block_in_place(|| rt.block_on(default_service())).expect("service init")
}

lazy_static! {
    static ref SERVICE: Service = default_service_blocking();
}

/// Domain errors are shown as they are; anything else is logged and hidden behind a generic text.
fn user_message(err: &AppError) -> String {
    match err.downcast_ref::<Err>() {
        Some(domain) => domain.to_string(),
        None => {
            log::error!("{}", err);
            "Something went wrong, please try again later".to_string()
        }
    }
}

/// Plain-text reply for both outcomes.
async fn reply(bot: &Bot, msg: &Message, result: Result<String, AppError>) -> ResponseResult<()> {
    let text = result.unwrap_or_else(|err| user_message(&err));
    bot.send_message(msg.chat.id, text)
        .disable_notification(true)
        .await?;
    Ok(())
}

/// Success is pre-escaped MarkdownV2, errors are plain text so their content can never break parsing.
async fn reply_markdown(
    bot: &Bot,
    msg: &Message,
    result: Result<String, AppError>,
) -> ResponseResult<()> {
    match result {
        Ok(text) => {
            bot.send_message(msg.chat.id, text)
                .parse_mode(MarkdownV2)
                .disable_web_page_preview(true)
                .disable_notification(true)
                .await?
        }
        Err(err) => {
            bot.send_message(msg.chat.id, user_message(&err))
                .disable_notification(true)
                .await?
        }
    };
    Ok(())
}

async fn command_handler(bot: Bot, msg: Message, cmd: Command) -> ResponseResult<()> {
    match cmd {
        Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .disable_notification(true)
                .await?;
            Ok(())
        }
        Command::Start => {
            let result = match SERVICE.register_new_club(msg.chat.id.0).await {
                Ok(()) => {
                    Ok("You're all set up! Now you can create event for your club".to_string())
                }
                Err(err)
                    if err
                        .downcast_ref::<tokio_postgres::Error>()
                        .and_then(|e| e.code())
                        == Some(&SqlState::UNIQUE_VIOLATION) =>
                {
                    Ok("You've already started a club".to_string())
                }
                Err(err) => Err(err),
            };
            reply(&bot, &msg, result).await
        }
        Command::Event(date) => {
            let date = date.trim();
            if date.is_empty() {
                return reply(
                    &bot,
                    &msg,
                    Ok("Please write a date in format -\n/event 2023.07.16 15:00".to_string()),
                )
                .await;
            }

            let result = SERVICE
                .new_club_event(msg.chat.id.0, date)
                .await
                .map(|date| format!("New club event created on {}", date));
            reply(&bot, &msg, result).await
        }
        Command::Suggest(suggestion) => {
            let suggestion = suggestion.trim();
            if suggestion.is_empty() {
                return reply(
                    &bot,
                    &msg,
                    Ok("Your suggestion is empty ;(\nFormat - /suggest smth".to_string()),
                )
                .await;
            }

            let result = match msg.from() {
                Some(user) => SERVICE
                    .new_member_suggestion(msg.chat.id.0, user.id.0, suggestion)
                    .await
                    .map(|()| format!("Got it. Your suggestion:\n{}", suggestion)),
                None => Err(Err::UnknownSender.into()),
            };
            reply(&bot, &msg, result).await
        }
        Command::Insights => {
            let result = SERVICE.toggle_with_insights(msg.chat.id.0).await;
            reply(&bot, &msg, result).await
        }
        Command::StartClub => {
            let result = SERVICE.start_active_event(msg.chat.id.0).await;
            reply_markdown(&bot, &msg, result).await
        }
        Command::Achieve => {
            let result = SERVICE
                .achieve_active_event(msg.chat.id.0)
                .await
                .map(|date| format!("Ok, event on {} is achieved", date));
            reply(&bot, &msg, result).await
        }
        Command::Pick => {
            let result = SERVICE.pick_from_suggestions(msg.chat.id.0).await;
            reply_markdown(&bot, &msg, result).await
        }
        Command::Current => {
            let result = SERVICE.get_current_event_info(msg.chat.id.0).await;
            reply_markdown(&bot, &msg, result).await
        }
    }
}

pub async fn run() {
    dotenv().ok();
    pretty_env_logger::init();

    let bot = Bot::from_env();

    bot.set_my_commands(Command::bot_commands())
        .await
        .expect("Failed to set bot commands");

    let handler = dptree::entry().branch(
        Update::filter_message()
            .filter_command::<Command>()
            .endpoint(command_handler),
    );

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
