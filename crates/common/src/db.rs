//! One way to open this project's SQLite database, and one way to change
//! its schema.
//!
//! Six modules opened the shared file with a 5-second busy timeout and WAL;
//! `telegram` and `miniapp` opened a fresh connection per call with neither,
//! which is exactly the configuration that produced a `SQLITE_BUSY`
//! incident. `open` is now the only door.

use rusqlite::Connection;

/// Open the shared database with the settings every handle needs: a busy
/// timeout so concurrent writers wait instead of failing, and WAL so readers
/// proceed during writes (persistent — set once, inherited by all handles).
pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    Ok(conn)
}

/// Add a column that a later release introduced.
///
/// This used to be written as `let _ = conn.execute("ALTER TABLE …")` in 32
/// places: the error was discarded because "duplicate column name" is the
/// expected outcome on an existing database — which also silently discarded
/// every *real* failure. Here the already-applied case is decided by asking
/// the schema, so anything else is a genuine error and propagates.
pub fn add_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    if has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

/// Whether `table` already has `column` (false when the table is absent).
pub fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        if r.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The schema version stamped on the database, so it is possible to tell
/// what a given file is. Read it with `sqlite3 pass-v1.db 'PRAGMA user_version'`.
pub const SCHEMA_VERSION: i64 = 22;

/// Stamp `SCHEMA_VERSION` once every module has created its tables.
pub fn stamp_version(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
}

/// The version this database carries (0 on anything stamped before this
/// existed).
pub fn version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}


/// Migration 1: the schema as of 2026-08-09 — every table this project has,
/// in one ordered place instead of eight `open()` functions.
///
/// Every statement is idempotent (`IF NOT EXISTS`, and `add_column` asks the
/// schema first), so running it against a database that already has the
/// schema is a no-op. That is what makes it safe to introduce against a live
/// file: production is already stamped at version 1 and skips it entirely,
/// while an unstamped database with the same tables would survive it anyway.
const SCHEMA_V1: &[&str] = &[
    // store_0
    r#"CREATE TABLE IF NOT EXISTS api_keys (
                id                 TEXT PRIMARY KEY,
                name               TEXT NOT NULL,
                key_hash           TEXT NOT NULL,
                rate_limit_per_min INTEGER NOT NULL,
                mode               TEXT NOT NULL DEFAULT 'hint',
                quality_tier       TEXT NOT NULL DEFAULT 'balanced',
                account_id         TEXT NOT NULL DEFAULT 'acc_default',
                cache_enabled      INTEGER NOT NULL DEFAULT 1,
                cache_ttl_secs     INTEGER,
                budget_monthly_micros INTEGER,
                created_at         TEXT NOT NULL DEFAULT (datetime('now')),
                revoked_at         TEXT,
                last_used_at       TEXT
            );
            CREATE TABLE IF NOT EXISTS execution_plans (
                plan_id    TEXT PRIMARY KEY,
                key_id     TEXT NOT NULL,
                plan_json  TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS owners (
                email         TEXT PRIMARY KEY,
                account_id    TEXT NOT NULL DEFAULT 'acc_default',
                verified      INTEGER NOT NULL DEFAULT 0,
                verify_token  TEXT,
                token_expires TEXT,
                created_at    TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS sessions (
                token_hash TEXT PRIMARY KEY,
                email      TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS nudges (
                account_id TEXT NOT NULL,
                kind       TEXT NOT NULL,
                sent_at    TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (account_id, kind)
            );
            CREATE TABLE IF NOT EXISTS invites (
                account_id TEXT PRIMARY KEY,
                code       TEXT UNIQUE NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS invite_claims (
                invitee_email   TEXT PRIMARY KEY,
                inviter_account TEXT NOT NULL,
                credited        INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS account_settings (
                account_id      TEXT PRIMARY KEY,
                soul_enabled    INTEGER NOT NULL DEFAULT 1,
                soul_text       TEXT,
                telegram_memory INTEGER NOT NULL DEFAULT 0,
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS chats (
                account_id TEXT NOT NULL,
                id         TEXT NOT NULL,
                title      TEXT NOT NULL,
                messages   TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (account_id, id)
            );"#,
    // registry
    r#"CREATE TABLE IF NOT EXISTS registry_models (
                id                        TEXT PRIMARY KEY,
                prompt_micros_per_mtok    INTEGER NOT NULL,
                completion_micros_per_mtok INTEGER NOT NULL,
                context                   INTEGER,
                structured_outputs        INTEGER NOT NULL DEFAULT 0,
                tools                     INTEGER NOT NULL DEFAULT 0,
                reasoning                 INTEGER NOT NULL DEFAULT 0,
                state                     TEXT NOT NULL DEFAULT 'trial',
                canary_pct                INTEGER NOT NULL DEFAULT 0,
                state_since               TEXT NOT NULL DEFAULT (datetime('now')),
                first_seen                TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at                TEXT NOT NULL DEFAULT (datetime('now')),
                missing_since             TEXT
            );
            CREATE TABLE IF NOT EXISTS model_aliases (
                alias  TEXT PRIMARY KEY,
                target TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS registry_alerts (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                message    TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS shadow_results (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                model      TEXT NOT NULL,
                task_type  TEXT NOT NULL,
                score      REAL NOT NULL,
                latency_ms INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_shadow_model ON shadow_results(model, created_at);
            CREATE TABLE IF NOT EXISTS golden_candidates (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                task_type  TEXT NOT NULL,
                prompt     TEXT NOT NULL,
                answer     TEXT NOT NULL,
                model      TEXT NOT NULL,
                score      REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS golden_set (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                task_type  TEXT NOT NULL,
                prompt     TEXT NOT NULL,
                answer     TEXT NOT NULL,
                model      TEXT NOT NULL,
                score      REAL,
                built_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );"#,
    // byok
    r#"CREATE TABLE IF NOT EXISTS account_keys (
                account_id     TEXT NOT NULL,
                provider       TEXT NOT NULL,
                nonce          BLOB NOT NULL,
                key_ct         BLOB NOT NULL,
                last4          TEXT NOT NULL,
                validated_at   TEXT,
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (account_id, provider)
            );"#,
    // lib_0
    r#"CREATE TABLE IF NOT EXISTS requests (
                id                   TEXT PRIMARY KEY,
                plan_id              TEXT NOT NULL,
                account_id           TEXT NOT NULL,
                key_id               TEXT NOT NULL,
                task_type            TEXT NOT NULL,
                difficulty           TEXT NOT NULL,
                status               TEXT NOT NULL,
                cache_status         TEXT NOT NULL,
                model_final          TEXT NOT NULL,
                route                TEXT NOT NULL,
                tokens_in            INTEGER NOT NULL,
                tokens_out           INTEGER NOT NULL,
                cost_micros          INTEGER NOT NULL,
                baseline_cost_micros INTEGER NOT NULL,
                latency_ms           INTEGER NOT NULL,
                prompt_excerpt       TEXT NOT NULL DEFAULT '',
                verification_score   REAL,
                ttft_ms              INTEGER,
                escalations          INTEGER NOT NULL DEFAULT 0,
                verification_passed  INTEGER,
                retry_detected       INTEGER NOT NULL DEFAULT 0,
                regenerated          INTEGER NOT NULL DEFAULT 0,
                created_at           TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_requests_created ON requests(created_at);
            CREATE INDEX IF NOT EXISTS idx_requests_account ON requests(account_id, created_at);
            CREATE TABLE IF NOT EXISTS routing_profiles (
                account_id TEXT NOT NULL DEFAULT '*',
                task_type  TEXT NOT NULL,
                lang       TEXT NOT NULL DEFAULT '*',
                model      TEXT NOT NULL,
                avg_score  REAL NOT NULL,
                samples    INTEGER NOT NULL,
                successes  REAL NOT NULL DEFAULT 0,
                failures   REAL NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (account_id, task_type, lang, model)
            );
            CREATE TABLE IF NOT EXISTS quality_scores (
                day   TEXT PRIMARY KEY,
                score REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS account_billing (
                account_id       TEXT PRIMARY KEY,
                allowance_micros INTEGER,
                paid             INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS tuning_spend (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id  TEXT NOT NULL,
                kind        TEXT NOT NULL,
                cost_micros INTEGER NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_tuning_account ON tuning_spend(account_id, created_at);
            CREATE TABLE IF NOT EXISTS recent_requests (
                ledger_id   TEXT PRIMARY KEY,
                key_id      TEXT NOT NULL,
                fingerprint BLOB NOT NULL,
                embedding   BLOB NOT NULL,
                entities    TEXT NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_recent_key ON recent_requests(key_id, created_at);"#,
    // lib
    r#"CREATE TABLE IF NOT EXISTS knowledge_bases (
                id         TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                name       TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS documents (
                id             TEXT PRIMARY KEY,
                kb_id          TEXT NOT NULL,
                filename       TEXT NOT NULL,
                size_bytes     INTEGER NOT NULL,
                status         TEXT NOT NULL,
                failure_reason TEXT,
                chunk_count    INTEGER NOT NULL DEFAULT 0,
                created_at     TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS key_knowledge_bases (
                key_id TEXT NOT NULL,
                kb_id  TEXT NOT NULL,
                PRIMARY KEY (key_id, kb_id)
            );"#,
    // telegram
    r#"CREATE TABLE IF NOT EXISTS telegram_link_codes (
            account_id TEXT PRIMARY KEY,
            code       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS telegram_offsets (
            account_id TEXT PRIMARY KEY,
            next_offset INTEGER NOT NULL
        );"#,
    // rerank_shadow
    r#"CREATE TABLE IF NOT EXISTS reranker_shadow (
            ledger_id    TEXT PRIMARY KEY,
            account_id   TEXT NOT NULL,
            task_type    TEXT NOT NULL,
            lang         TEXT NOT NULL,
            n_sources    INTEGER NOT NULL,
            n_sentences  INTEGER NOT NULL,
            local_score  REAL,
            remote_score REAL,
            remote_ms    INTEGER,
            error        TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );"#,
];


/// Columns later releases added to migration 1's tables.
const COLUMNS_V1: &[(&str, &str, &str)] = &[
    ("chats", "memory", "INTEGER NOT NULL DEFAULT 0"),
    ("owners", "account_id", r#"TEXT NOT NULL DEFAULT 'acc_default'"#),
    ("owners", "token_expires", "TEXT"),
    ("owners", "approved", "INTEGER NOT NULL DEFAULT 0"),
    ("api_keys", "mode", r#"TEXT NOT NULL DEFAULT 'hint'"#),
    ("api_keys", "quality_tier", r#"TEXT NOT NULL DEFAULT 'balanced'"#),
    ("api_keys", "account_id", r#"TEXT NOT NULL DEFAULT 'acc_default'"#),
    ("api_keys", "cache_enabled", "INTEGER NOT NULL DEFAULT 1"),
    ("api_keys", "cache_ttl_secs", "INTEGER"),
    ("api_keys", "budget_monthly_micros", "INTEGER"),
    ("account_settings", "telegram_web", "INTEGER NOT NULL DEFAULT 1"),
    ("account_settings", "telegram_miniapp", "INTEGER NOT NULL DEFAULT 0"),
    ("account_settings", "telegram_open", "INTEGER NOT NULL DEFAULT 0"),
    ("account_settings", "telegram_stream", "INTEGER NOT NULL DEFAULT 1"),
    ("registry_models", "canary_pct", "INTEGER NOT NULL DEFAULT 0"),
    ("registry_models", "state_since", r#"TEXT NOT NULL DEFAULT ''"#),
    ("requests", "verification_score", "REAL"),
    ("requests", "ttft_ms", "INTEGER"),
    ("requests", "escalations", "INTEGER NOT NULL DEFAULT 0"),
    ("requests", "verification_passed", "INTEGER"),
    ("requests", "retry_detected", "INTEGER NOT NULL DEFAULT 0"),
    ("requests", "regenerated", "INTEGER NOT NULL DEFAULT 0"),
    ("requests", "internal", "INTEGER NOT NULL DEFAULT 0"),
    ("requests", "lang", r#"TEXT NOT NULL DEFAULT 'en'"#),
    ("requests", "house_fallback", "INTEGER NOT NULL DEFAULT 0"),
    ("requests", "retry_sim", "REAL"),
    ("requests", "origin", r#"TEXT NOT NULL DEFAULT 'api'"#),
    ("requests", "web_search", "INTEGER NOT NULL DEFAULT 0"),
    ("routing_profiles", "successes", "REAL NOT NULL DEFAULT 0"),
    ("routing_profiles", "failures", "REAL NOT NULL DEFAULT 0"),
];

/// Migration 2 (card 3512): the account's memory-scope setting (shared vs
/// per-person). The former `memories.scope_key` column is retired — memory
/// storage moved to Pass Tools, and the scope now rides the tools metadata.
const COLUMNS_V2: &[(&str, &str, &str)] = &[(
    "account_settings",
    "memory_scope",
    r#"TEXT NOT NULL DEFAULT 'shared'"#,
)];

/// Migration 3: retired. It added `memories.author`; the `memories` table is
/// gone (memory lives in Pass Tools). Kept as an empty step so the version
/// sequence and any database already past it are undisturbed.
const COLUMNS_V3: &[(&str, &str, &str)] = &[];

/// Migration 4: a Telegram bot can hide the receipt line under its answers.
const COLUMNS_V4: &[(&str, &str, &str)] = &[(
    "account_settings",
    "telegram_hide_receipt",
    "INTEGER NOT NULL DEFAULT 0",
)];

/// Migration 5: per-provider monthly spend limits. The account watches what
/// it spends through each connected provider and stops using one that hits
/// its cap for the month.
const SCHEMA_V5: &[&str] = &[r#"CREATE TABLE IF NOT EXISTS provider_limits (
    account_id           TEXT NOT NULL,
    provider             TEXT NOT NULL,
    monthly_limit_micros INTEGER NOT NULL,
    PRIMARY KEY (account_id, provider)
);"#];

/// Migration 6: the Pass Tools usage feed, pulled and stored locally.
///
/// One row per unit of tool work the platform did for an account (search,
/// fetch, crawl, …). `id` is the tools service's own append-only row id, so
/// it doubles as the poll cursor — `MAX(id)` is where the next pull resumes,
/// with no separate cursor to keep in sync, and re-seeing a row is a no-op
/// (`INSERT OR IGNORE`). `provider` is null for free local work and names the
/// paid upstream (e.g. `serper`) when the tools spent real money; units are
/// work done, not calls.
const SCHEMA_V6: &[&str] = &[r#"CREATE TABLE IF NOT EXISTS tool_usage (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,
    account    TEXT NOT NULL,
    verb       TEXT NOT NULL,
    provider   TEXT,
    units      INTEGER NOT NULL DEFAULT 0,
    cached     INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tool_usage_acct ON tool_usage(account, ts);"#];

/// Migration 7: mark when an account's Telegram bot token has been handed to
/// Pass Tools, so the gate registers it exactly once (a re-register would
/// clear the tools' offset and risk replaying the inbox on every restart).
const COLUMNS_V7: &[(&str, &str, &str)] =
    &[("account_settings", "tg_registered", "INTEGER NOT NULL DEFAULT 0")];

/// Migration 8: drop the local `memories` and `chunks` tables. Memory and KB
/// vectors live entirely in Pass Tools now; these were dead duplicates (their
/// data already copied to the tools). Existing databases drop them here; fresh
/// ones never create them.
const SCHEMA_V8: &[&str] = &[
    "DROP TABLE IF EXISTS memories;",
    "DROP TABLE IF EXISTS chunks;",
];

/// Migration 9: drop dead tables. `invoices`/`billed_requests` backed an
/// invoicing model that never shipped (charging came later, and per request
/// against the wallet rather than by invoice); `telegram_chats`
/// is superseded by pass-tools, which owns linked chats now. Existing databases
/// drop them here; fresh ones never create them.
const SCHEMA_V9: &[&str] = &[
    "DROP TABLE IF EXISTS invoices;",
    "DROP TABLE IF EXISTS billed_requests;",
    "DROP TABLE IF EXISTS telegram_chats;",
];

/// V10: Pass now charges a cent per request, so an account with an empty
/// wallet cannot make one. Every account that already exists gets the same
/// $10 a new signup gets — otherwise the release would lock out every
/// existing user, the demo account included, the moment it shipped. Accounts
/// are discovered from `owners` and from the keys already issued, because an
/// account can exist without ever having signed up through the console.
const SCHEMA_V10: &[&str] = &[
    "INSERT INTO account_billing (account_id, allowance_micros, paid)
     SELECT DISTINCT account_id, 10000000, 0 FROM owners
     WHERE account_id IS NOT NULL
     ON CONFLICT(account_id) DO NOTHING;",
    "INSERT INTO account_billing (account_id, allowance_micros, paid)
     SELECT DISTINCT account_id, 10000000, 0 FROM api_keys
     WHERE account_id IS NOT NULL
     ON CONFLICT(account_id) DO NOTHING;",
];

/// V11: recorded top-up intents. A user who entered an amount and pressed the
/// button told us what they would have paid, and that number is the only
/// evidence we have about pricing the till before the till exists. Kept even
/// though no payment can follow yet — especially because none can.
const SCHEMA_V11: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS topup_intents (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         account_id TEXT NOT NULL,
         amount_micros INTEGER NOT NULL,
         created_at TEXT NOT NULL DEFAULT (datetime('now'))
     );",
    "CREATE INDEX IF NOT EXISTS topup_intents_account
         ON topup_intents(account_id, created_at DESC);",
];

/// V12: one sentence per KB document, written by the small model at upload.
/// It serves two readers: the compiler's system note (choosing between the
/// knowledge base and the open web needs to know what the base HOLDS), and
/// the contextual prefix on every chunk (Anthropic's contextual retrieval).
const COLUMNS_V12: &[(&str, &str, &str)] = &[("documents", "description", "TEXT")];

/// V13 (identity spec §1-2): sessions carry an origin label and a birthday so
/// the Console can list and revoke them by name; the pending magic link
/// remembers who asked for it, so the confirmation page can say so.
const COLUMNS_V13: &[(&str, &str, &str)] = &[
    ("sessions", "origin", "TEXT"),
    ("sessions", "created_at", "TEXT"),
    ("owners", "token_origin", "TEXT"),
];

/// The V13 table half: magic-link request throttling needs a log to count.
/// V14 (identity spec §5-8): every owner is a citizen — a username, a species
/// (human|agent), and, for agents, the account that owns them. Usernames are
/// unique case-insensitively; the app validates shape and reserved words
/// before insert. Nullable so existing owners migrate without a value and
/// claim a username on next sign-in.
const COLUMNS_V14: &[(&str, &str, &str)] = &[
    ("owners", "username", "TEXT"),
    ("owners", "kind", "TEXT"),
    ("owners", "owned_by", "TEXT"),
    ("owners", "display_name", "TEXT"),
    ("owners", "bio", "TEXT"),
];

const SCHEMA_V14: &[&str] = &[
    // Case-insensitive uniqueness by construction (identity spec §6).
    "CREATE UNIQUE INDEX IF NOT EXISTS owners_username
         ON owners(lower(username)) WHERE username IS NOT NULL;",
    // The reserved list lives in a table so it updates without a deploy (§5).
    "CREATE TABLE IF NOT EXISTS reserved_usernames (
         name TEXT PRIMARY KEY
     );",
    "INSERT OR IGNORE INTO reserved_usernames(name) VALUES
         ('index'),('chat'),('desk'),('console'),('market'),('node'),('tools'),
         ('models'),('api'),('docs'),('status'),('admin'),('root'),('support'),
         ('help'),('official'),('pass'),('passbot'),('billing'),('security'),
         ('abuse'),('postmaster'),('noreply'),('signin'),('verify'),('join'),
         ('healthz'),('v1'),('tg'),('mcp');",
];

const SCHEMA_V13: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS login_requests (
         email TEXT NOT NULL,
         requested_at TEXT NOT NULL DEFAULT (datetime('now'))
     );",
    "CREATE INDEX IF NOT EXISTS login_requests_email
         ON login_requests(email, requested_at);",
];

/// V19: cross-device magic-link. The requesting browser gets a poll token
/// (`rt`) at sign-in; when the link is confirmed on ANY device the matching
/// login_requests row flips `verified`, and the requester's poll mints a
/// session for it too — sign in on the TV, confirm on the phone, both land in.
const COLUMNS_V19: &[(&str, &str, &str)] = &[
    ("login_requests", "rt", "TEXT"),
    ("login_requests", "origin", "TEXT"),
    ("login_requests", "verified", "INTEGER NOT NULL DEFAULT 0"),
    ("login_requests", "consumed", "INTEGER NOT NULL DEFAULT 0"),
];

/// V20: the magic-link token that was emailed for THIS request. Confirming a
/// link now verifies only the request carrying that exact token, not every
/// pending request for the email — so an attacker's parallel request for a
/// victim's address can no longer ride the victim's own confirmation into a
/// session (account-takeover fix).
const COLUMNS_V20: &[(&str, &str, &str)] = &[
    ("login_requests", "verify_token", "TEXT"),
];

/// V21: Inbox (agent-to-agent messenger) MVP — a room is a shareable link, one
/// shared message thread, joined by a Pass session. The host account owns the
/// room. (A2A/MCP doors, three-word guest handles, message signing and
/// host-pays metering are later layers.)
const SCHEMA_V21: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS rooms (
         id           TEXT PRIMARY KEY,
         host_account TEXT NOT NULL,
         title        TEXT NOT NULL DEFAULT '',
         created_at   TEXT NOT NULL DEFAULT (datetime('now'))
     );",
    "CREATE TABLE IF NOT EXISTS room_members (
         room_id   TEXT NOT NULL,
         account   TEXT NOT NULL,
         display   TEXT NOT NULL DEFAULT '',
         joined_at TEXT NOT NULL DEFAULT (datetime('now')),
         PRIMARY KEY (room_id, account)
     );",
    "CREATE TABLE IF NOT EXISTS room_messages (
         id             INTEGER PRIMARY KEY AUTOINCREMENT,
         room_id        TEXT NOT NULL,
         sender_account TEXT NOT NULL,
         sender_display TEXT NOT NULL DEFAULT '',
         body           TEXT NOT NULL,
         created_at     TEXT NOT NULL DEFAULT (datetime('now'))
     );",
    "CREATE INDEX IF NOT EXISTS room_messages_room ON room_messages(room_id, id);",
];

/// V22: Inbox guests — a stranger without a Pass account walks into a room by
/// its link as a memorable three-word handle (`@big-red-apple`), held by a
/// browser cookie token. The guest is also a `room_members` row (account =
/// `g_<token-prefix>`), so posting and reading need no special-casing beyond
/// resolving the cookie to that account.
const SCHEMA_V22: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS room_guests (
         token      TEXT PRIMARY KEY,
         room_id    TEXT NOT NULL,
         handle     TEXT NOT NULL,
         account    TEXT NOT NULL,
         created_at TEXT NOT NULL DEFAULT (datetime('now'))
     );",
];

/// Bring `conn`'s database up to `SCHEMA_VERSION`.
///
/// The single ordered migration this project runs. Gated on
/// `PRAGMA user_version`, so an up-to-date database does no work; anything
/// older applies the missing steps in order and fails loudly if a step
/// genuinely fails.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let at = version(conn)?;
    if at >= SCHEMA_VERSION {
        return Ok(());
    }
    if at < 1 {
        for batch in SCHEMA_V1 {
            conn.execute_batch(batch)?;
        }
        for (table, column, definition) in COLUMNS_V1 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 2 {
        for (table, column, definition) in COLUMNS_V2 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 3 {
        for (table, column, definition) in COLUMNS_V3 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 4 {
        for (table, column, definition) in COLUMNS_V4 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 5 {
        for batch in SCHEMA_V5 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 6 {
        for batch in SCHEMA_V6 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 7 {
        for (table, column, definition) in COLUMNS_V7 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 8 {
        for batch in SCHEMA_V8 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 9 {
        for batch in SCHEMA_V9 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 10 {
        for batch in SCHEMA_V10 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 11 {
        for batch in SCHEMA_V11 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 12 {
        for (table, column, definition) in COLUMNS_V12 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 13 {
        for (table, column, definition) in COLUMNS_V13 {
            add_column(conn, table, column, definition)?;
        }
        for batch in SCHEMA_V13 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 14 {
        for (table, column, definition) in COLUMNS_V14 {
            add_column(conn, table, column, definition)?;
        }
        for batch in SCHEMA_V14 {
            conn.execute_batch(batch)?;
        }
    }
    if at < 15 {
        // World brand names + common words reserved so nobody squats them
        // (Eugene, 2026-08-29). ONE transaction, not 10k autocommits — a
        // per-row fsync here timed out the test suite's fresh DBs.
        conn.execute_batch("BEGIN")?;
        {
            let mut stmt =
                conn.prepare("INSERT OR IGNORE INTO reserved_usernames(name) VALUES (?1)")?;
            for src in [
                include_str!("../data/brands.txt"),
                include_str!("../data/words.txt"),
            ] {
                for name in src.lines() {
                    let name = name.trim();
                    if !name.is_empty() {
                        stmt.execute([name])?;
                    }
                }
            }
        }
        conn.execute_batch("COMMIT")?;
    }
    if at < 16 {
        // Corporate profiles at pass.io/{domain}: the AI-generated card
        // (fetched from the homepage) keyed by domain; the profile shows the
        // bare domain + members until it fills in.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS org_cards (
                 domain      TEXT PRIMARY KEY,
                 name        TEXT,
                 description TEXT,
                 category    TEXT,
                 logo_url    TEXT,
                 website     TEXT,
                 index_slug  TEXT,
                 claimed_by  TEXT,
                 fetched_at  TEXT,
                 updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
    }
    if at < 17 {
        // Every landing on /signin, with the params that led there — the
        // whole site now funnels here, so where a sign-in came from is worth
        // keeping (Eugene, 2026-08-29).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS signin_visits (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 params     TEXT NOT NULL DEFAULT '',
                 referer    TEXT NOT NULL DEFAULT '',
                 user_agent TEXT NOT NULL DEFAULT '',
                 at         TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE INDEX IF NOT EXISTS signin_visits_at ON signin_visits(at DESC);",
        )?;
    }
    if at < 18 {
        // The wallet's funding journal (Eugene, 2026-08-30): one row per
        // discrete credit — opening grant, invite bonus, top-up, and ahead the
        // social earnings (review, upvote, action). `micros` is signed so a
        // manual debit can be recorded too, but the per-request cent fee is
        // NOT journaled here — that would be millions of rows and already lives
        // in the request ledger. This is "how the wallet was funded", shown as
        // history in Settings → Wallet.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS credit_events (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 account_id TEXT NOT NULL,
                 kind       TEXT NOT NULL,
                 micros     INTEGER NOT NULL,
                 meta       TEXT NOT NULL DEFAULT '',
                 at         TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE INDEX IF NOT EXISTS credit_events_account
                 ON credit_events(account_id, id DESC);",
        )?;
    }
    if at < 19 {
        for (table, column, definition) in COLUMNS_V19 {
            add_column(conn, table, column, definition)?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS login_requests_rt ON login_requests(rt);",
        )?;
    }
    if at < 20 {
        for (table, column, definition) in COLUMNS_V20 {
            add_column(conn, table, column, definition)?;
        }
    }
    if at < 21 {
        for stmt in SCHEMA_V21 {
            conn.execute_batch(stmt)?;
        }
    }
    if at < 22 {
        for stmt in SCHEMA_V22 {
            conn.execute_batch(stmt)?;
        }
    }
    stamp_version(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_column_is_idempotent_and_loud() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a TEXT)").unwrap();
        add_column(&conn, "t", "b", "INTEGER NOT NULL DEFAULT 0").unwrap();
        assert!(has_column(&conn, "t", "b").unwrap());
        // Running it again is a no-op, not an error.
        add_column(&conn, "t", "b", "INTEGER NOT NULL DEFAULT 0").unwrap();
        // A real failure is no longer swallowed.
        assert!(add_column(&conn, "missing_table", "c", "TEXT").is_err());
    }

    #[test]
    fn version_round_trips() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(version(&conn).unwrap(), 0);
        stamp_version(&conn).unwrap();
        assert_eq!(version(&conn).unwrap(), SCHEMA_VERSION);
    }
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    /// A fresh file gets the whole schema and lands on the current version.
    #[test]
    fn empty_database_is_built_from_nothing() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(version(&conn).unwrap(), SCHEMA_VERSION);
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(tables >= 30, "expected the full schema, got {tables} tables");
        // Columns added by later releases are there too.
        assert!(has_column(&conn, "requests", "web_search").unwrap());
        assert!(has_column(&conn, "api_keys", "account_id").unwrap());
    }

    /// The case that makes this safe to ship against production: a database
    /// that already has the schema but was never stamped must survive the
    /// migration untouched, because every statement in it is idempotent.
    #[test]
    fn existing_unstamped_database_survives() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch("INSERT INTO owners (email) VALUES ('a@b.c')")
            .ok();
        conn.pragma_update(None, "user_version", 0).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(version(&conn).unwrap(), SCHEMA_VERSION);
        let owners: i64 = conn
            .query_row("SELECT count(*) FROM owners", [], |r| r.get(0))
            .unwrap();
        assert_eq!(owners, 1, "existing rows must not be disturbed");
    }

    /// An up-to-date database does no work at all.
    #[test]
    fn current_database_is_a_no_op() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch("DROP TABLE owners").unwrap();
        // Still at the current version, so migrate must not rebuild it.
        migrate(&conn).unwrap();
        assert!(!has_column(&conn, "owners", "email").unwrap());
    }
}

