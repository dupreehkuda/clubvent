CREATE TABLE IF NOT EXISTS "club" (
    "chat_id" int8 PRIMARY KEY NOT NULL,
    "last_event" timestamp,
    "next_event" timestamp,
    "created_at" timestamptz NOT NULL DEFAULT NOW(),
    "active_event" uuid
);

CREATE TABLE IF NOT EXISTS "events" (
    "id" uuid PRIMARY KEY NOT NULL,
    "chat_id" int8 NOT NULL REFERENCES "club" ("chat_id"),
    "subject" text,
    "active" bool,
    "event_date" timestamptz NOT NULL,
    "achieved_on" timestamptz,
    "created_at" timestamptz NOT NULL DEFAULT NOW(),
    "insights" boolean NOT NULL DEFAULT false,
    "insights_link" text
);

CREATE TABLE IF NOT EXISTS "suggestions" (
    "event_id" uuid NOT NULL REFERENCES "events" ("id"),
    "chat_id" int8 NOT NULL,
    "user_id" int8 NOT NULL,
    "suggestion" text,
    "created_at" timestamptz NOT NULL DEFAULT NOW()
);
