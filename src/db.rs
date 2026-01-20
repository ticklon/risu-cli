use crate::config;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Note {
    pub id: String,
    pub content: String,
    pub updated_at: String,
    pub is_deleted: i32,
    #[serde(default)]
    pub is_synced: i32,
    #[serde(default)]
    pub is_encrypted: i32,
}

pub enum DbRequest {
    GetNotes {
        reply: oneshot::Sender<Result<Vec<Note>>>,
    },
    GetNote {
        id: String,
        reply: oneshot::Sender<Result<Option<Note>>>,
    },
    SaveNote {
        id: Option<String>,
        content: String,
        is_encrypted: bool,
        reply: oneshot::Sender<Result<String>>,
    },
    DeleteNote {
        id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    GetUnsyncedNotes {
        reply: oneshot::Sender<Result<Vec<Note>>>,
    },
    MarkAsSynced {
        id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    PullUpsertNotes {
        notes: Vec<Note>,
        cursor: String,
        reply: oneshot::Sender<Result<()>>,
    },
    GetKV {
        key: String,
        reply: oneshot::Sender<Result<Option<String>>>,
    },
    SetKV {
        key: String,
        value: String,
        reply: oneshot::Sender<Result<()>>,
    },
    DeleteKV {
        key: String,
        reply: oneshot::Sender<Result<()>>,
    },
    #[allow(dead_code)]
    ClearAllData { reply: oneshot::Sender<Result<()>> },
    SetNotesEncryptedStatus {
        is_encrypted: i32,
        reply: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
pub struct Repo {
    tx: mpsc::UnboundedSender<DbRequest>,
}

impl Repo {
    pub fn new() -> Result<Self> {
        // Initialize DB synchronously so we fail early if DB can't be created/opened.
        let mut actor = RepoInternal::new().context("Failed to initialize database actor")?;

        let (tx, rx) = mpsc::unbounded_channel();

        // Spawn the actor thread with the already initialized actor.
        std::thread::spawn(move || {
            actor.run(rx);
        });

        Ok(Self { tx })
    }

    pub async fn get_notes(&self) -> Result<Vec<Note>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::GetNotes { reply })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn get_note(&self, id: String) -> Result<Option<Note>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::GetNote { id, reply })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn save_note(
        &self,
        id: Option<String>,
        content: String,
        is_encrypted: bool,
    ) -> Result<String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::SaveNote {
                id,
                content,
                is_encrypted,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn delete_note(&self, id: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::DeleteNote { id, reply })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn get_unsynced_notes(&self) -> Result<Vec<Note>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::GetUnsyncedNotes { reply })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn mark_as_synced(&self, id: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::MarkAsSynced { id, reply })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn pull_upsert_notes(&self, notes: Vec<Note>, cursor: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::PullUpsertNotes {
                notes,
                cursor,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped.reply")?
    }

    // --- KV Store Helpers ---

    pub async fn get_kv(&self, key: &str) -> Result<Option<String>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::GetKV {
                key: key.to_string(),
                reply,
            })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::SetKV {
                key: key.to_string(),
                value: value.to_string(),
                reply,
            })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn get_cursor(&self) -> Result<String> {
        self.get_kv("last_synced_at")
            .await
            .map(|v| v.unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()))
    }

    pub async fn set_last_synced(&self, cursor: &str) -> Result<()> {
        self.set_kv("last_synced_at", cursor).await
    }

    pub async fn get_salt(&self) -> Result<Option<String>> {
        self.get_kv("encryption_salt").await
    }

    pub async fn set_salt(&self, salt: &str) -> Result<()> {
        self.set_kv("encryption_salt", salt).await
    }

    pub async fn delete_kv(&self, key: &str) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::DeleteKV {
                key: key.to_string(),
                reply,
            })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn delete_salt(&self) -> Result<()> {
        self.delete_kv("encryption_salt").await
    }

    #[allow(dead_code)]
    pub async fn clear_all_data(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::ClearAllData { reply })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }

    pub async fn set_notes_encrypted_status(&self, is_encrypted: i32) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DbRequest::SetNotesEncryptedStatus {
                is_encrypted,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("DB actor shutdown"))?;
        rx.await.context("DB actor dropped reply")?
    }
}

// Synchronous internal implementation
struct RepoInternal {
    conn: Connection,
}

impl RepoInternal {
    fn new() -> Result<Self> {
        let config_dir = config::get_config_dir();
        std::fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let mut db_path = config_dir;
        db_path.push("local.db");

        let conn = Connection::open(db_path).context("Failed to open database")?;
        let internal = Self { conn };
        internal
            .create_tables()
            .context("Failed to create tables")?;
        Ok(internal)
    }

    fn create_tables(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                content TEXT,
                updated_at TEXT,
                is_deleted INTEGER DEFAULT 0,
                is_synced INTEGER DEFAULT 1,
                is_encrypted INTEGER DEFAULT 0
            );",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS kv_store (
                key TEXT PRIMARY KEY,
                value TEXT
            );",
            [],
        )?;

        Ok(())
    }

    fn run(&mut self, mut rx: mpsc::UnboundedReceiver<DbRequest>) {
        while let Some(req) = rx.blocking_recv() {
            match req {
                DbRequest::GetNotes { reply } => {
                    let _ = reply.send(self.get_notes());
                }
                DbRequest::GetNote { id, reply } => {
                    let _ = reply.send(self.get_note(&id));
                }
                DbRequest::SaveNote {
                    id,
                    content,
                    is_encrypted,
                    reply,
                } => {
                    let _ = reply.send(self.save_note(id, &content, is_encrypted));
                }
                DbRequest::DeleteNote { id, reply } => {
                    let _ = reply.send(self.delete_note(&id));
                }
                DbRequest::GetUnsyncedNotes { reply } => {
                    let _ = reply.send(self.get_unsynced_notes());
                }
                DbRequest::MarkAsSynced { id, reply } => {
                    let _ = reply.send(self.mark_as_synced(&id));
                }
                DbRequest::PullUpsertNotes {
                    notes,
                    cursor,
                    reply,
                } => {
                    let _ = reply.send(self.pull_upsert_notes(notes, &cursor));
                }
                DbRequest::GetKV { key, reply } => {
                    let _ = reply.send(self.get_kv(&key));
                }
                DbRequest::SetKV { key, value, reply } => {
                    let _ = reply.send(self.set_kv(&key, &value));
                }
                DbRequest::DeleteKV { key, reply } => {
                    let _ = reply.send(self.delete_kv(&key));
                }
                DbRequest::ClearAllData { reply } => {
                    let _ = reply.send(self.clear_all_data());
                }
                DbRequest::SetNotesEncryptedStatus {
                    is_encrypted,
                    reply,
                } => {
                    let _ = reply.send(self.set_notes_encrypted_status(is_encrypted));
                }
            }
        }
    }

    fn get_notes(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, updated_at, is_deleted, is_synced, is_encrypted

             FROM notes 

             WHERE is_deleted = 0

             ORDER BY updated_at DESC",
        )?;

        let note_iter = stmt.query_map([], |row| {
            Ok(Note {
                id: row.get(0)?,

                content: row.get(1)?,

                updated_at: row.get(2)?,

                is_deleted: row.get(3)?,

                is_synced: row.get(4)?,

                is_encrypted: row.get(5)?,
            })
        })?;

        let mut notes = Vec::new();

        for note in note_iter {
            notes.push(note?);
        }

        Ok(notes)
    }

    fn get_note(&self, id: &str) -> Result<Option<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, updated_at, is_deleted, is_synced, is_encrypted 

             FROM notes WHERE id = ?1",
        )?;

        let mut rows = stmt.query(params![id])?;

        if let Some(row) = rows.next()? {
            Ok(Some(Note {
                id: row.get(0)?,

                content: row.get(1)?,

                updated_at: row.get(2)?,

                is_deleted: row.get(3)?,

                is_synced: row.get(4)?,

                is_encrypted: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    fn save_note(&self, id: Option<String>, content: &str, is_encrypted: bool) -> Result<String> {
        let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

        let now = Utc::now().to_rfc3339();

        let encrypted_flag = if is_encrypted { 1 } else { 0 };

        self.conn.execute(
            "INSERT INTO notes (id, content, updated_at, is_deleted, is_synced, is_encrypted)

             VALUES (?1, ?2, ?3, 0, 0, ?4)

             ON CONFLICT(id) DO UPDATE SET

                content = excluded.content,

                updated_at = excluded.updated_at,

                is_deleted = 0,

                is_synced = 0,

                is_encrypted = excluded.is_encrypted",
            params![id, content, now, encrypted_flag],
        )?;

        Ok(id)
    }

    fn delete_note(&self, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            "UPDATE notes SET is_deleted = 1, is_synced = 0, updated_at = ?1 

             WHERE id = ?2",
            params![now, id],
        )?;

        Ok(())
    }

    fn get_unsynced_notes(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, updated_at, is_deleted, is_synced, is_encrypted 

             FROM notes WHERE is_synced = 0",
        )?;

        let note_iter = stmt.query_map([], |row| {
            Ok(Note {
                id: row.get(0)?,

                content: row.get(1)?,

                updated_at: row.get(2)?,

                is_deleted: row.get(3)?,

                is_synced: row.get(4)?,

                is_encrypted: row.get(5)?,
            })
        })?;

        let mut notes = Vec::new();

        for note in note_iter {
            notes.push(note?);
        }

        Ok(notes)
    }

    fn mark_as_synced(&self, id: &str) -> Result<()> {
        self.conn
            .execute("UPDATE notes SET is_synced = 1 WHERE id = ?", [id])?;

        Ok(())
    }

    fn pull_upsert_notes(&mut self, notes: Vec<Note>, cursor: &str) -> Result<()> {
        let tx = self.conn.transaction()?;

        for n in notes {
            tx.execute(
                "INSERT INTO notes (id, content, updated_at, is_deleted, is_synced, is_encrypted)

                 VALUES (?1, ?2, ?3, ?4, 1, ?5)

                 ON CONFLICT(id) DO UPDATE SET

                    content = excluded.content,

                    updated_at = excluded.updated_at,

                    is_deleted = excluded.is_deleted,

                    is_synced = 1,

                    is_encrypted = excluded.is_encrypted

                 WHERE excluded.updated_at > notes.updated_at",
                params![n.id, n.content, n.updated_at, n.is_deleted, n.is_encrypted],
            )?;
        }

        tx.execute(
            "INSERT OR REPLACE INTO kv_store (key, value) VALUES (?1, ?2)",
            params!["last_synced_at", cursor],
        )?;

        tx.commit()?;
        Ok(())
    }

    fn get_kv(&self, key: &str) -> Result<Option<String>> {
        let res: Result<String, rusqlite::Error> = self.conn.query_row(
            "SELECT value FROM kv_store WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );

        match res {
            Ok(val) => Ok(Some(val)),

            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),

            Err(e) => Err(e.into()),
        }
    }

    fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO kv_store (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;

        Ok(())
    }

    fn delete_kv(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM kv_store WHERE key = ?1", params![key])?;

        Ok(())
    }

    #[allow(dead_code)]
    fn clear_all_data(&self) -> Result<()> {
        self.conn.execute("DELETE FROM notes", [])?;

        self.conn.execute("DELETE FROM kv_store", [])?;

        Ok(())
    }

    fn set_notes_encrypted_status(&self, is_encrypted: i32) -> Result<()> {
        self.conn.execute(
            "UPDATE notes SET is_encrypted = ?1, is_synced = 0 

             WHERE is_deleted = 0",
            params![is_encrypted],
        )?;

        Ok(())
    }
}
