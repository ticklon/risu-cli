use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;
use zeroize::Zeroizing;

use crate::config;
use crate::crypto;
use crate::db::{Note, Repo};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Synced,
    Syncing,
    Offline,
    Error,
    Unlocking,
    Unlocked,
    PaymentRequired,
}

impl SyncStatus {
    pub fn as_str(&self) -> &str {
        match self {
            SyncStatus::Synced => "Synced",
            SyncStatus::Syncing => "Syncing...",
            SyncStatus::Offline => "Offline",
            SyncStatus::Error => "Error",
            SyncStatus::Unlocking => "Unlocking...",
            SyncStatus::Unlocked => "Unlocked",
            SyncStatus::PaymentRequired => "Upgrade Required",
        }
    }
}

pub struct APIClient {
    client: Client,
    base_url: String,
}

impl APIClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
            base_url: config::get_api_base_url(),
        }
    }

    async fn authenticated_request<T: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&T>,
    ) -> Result<Response> {
        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            attempts += 1;
            let url = format!("{}{}", self.base_url, path);
            let mut builder = self.client.request(method.clone(), &url);

            let token = config::get_token();
            if !token.is_empty() {
                builder = builder.bearer_auth(token);
            }

            if let Some(b) = body {
                builder = builder.json(b);
            }

            let res = builder.send().await;

            match res {
                Ok(resp) => {
                    if resp.status() == StatusCode::UNAUTHORIZED
                        && attempts == 1
                        && self.refresh_token().await.is_ok()
                    {
                        continue;
                    }

                    if resp.status().is_server_error() && attempts < max_attempts {
                        time::sleep(Duration::from_millis(500 * attempts)).await;
                        continue;
                    }

                    return Ok(resp);
                }
                Err(e) if attempts < max_attempts => {
                    time::sleep(Duration::from_millis(500 * attempts)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn refresh_token(&self) -> Result<()> {
        let data = config::get_token_data();
        if data.refresh_token.is_empty() {
            return Err(anyhow!("No refresh token"));
        }

        let resp = self
            .client
            .post(format!("{}/auth/refresh", self.base_url))
            .json(&serde_json::json!({ "refresh_token": data.refresh_token }))
            .send()
            .await?;

        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Refresh failed: {}", resp.status()));
        }

        #[derive(Deserialize)]
        struct RefreshRes {
            id_token: String,
            refresh_token: String,
        }
        let res: RefreshRes = resp.json().await?;
        config::save_token_data(&res.id_token, &res.refresh_token)?;
        Ok(())
    }

    pub async fn check_sync(&self) -> Result<String> {
        let resp = self
            .authenticated_request::<()>(Method::GET, "/sync/check", None)
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Sync check failed: {}", resp.status()));
        }
        #[derive(Deserialize)]
        struct CheckRes {
            last_updated_at: String,
        }
        let res: CheckRes = resp.json().await?;
        Ok(res.last_updated_at)
    }

    pub async fn pull_changes(&self, since: &str) -> Result<PullResult> {
        let path = format!("/sync/pull?since={}", since);
        let resp = self
            .authenticated_request::<()>(Method::GET, &path, None)
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Pull failed: {}", resp.status()));
        }
        let res: PullResult = resp.json().await?;
        Ok(res)
    }

    pub async fn push_note(&self, note: &Note) -> Result<()> {
        let resp = self
            .authenticated_request(Method::POST, "/sync/push", Some(note))
            .await?;

        if resp.status() == StatusCode::FORBIDDEN {
            return Err(anyhow!("Payment Required"));
        }

        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Push failed: {}", resp.status()));
        }
        Ok(())
    }

    pub async fn start_login_session(&self) -> Result<LoginSession> {
        let resp = self
            .client
            .post(format!("{}/auth/init", self.base_url))
            .send()
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Init failed: {}", resp.status()));
        }
        let session: LoginSession = resp.json().await?;
        Ok(session)
    }

    pub async fn poll_login_session(&self, session_id: &str) -> Result<PollResult> {
        let poll_url = format!("{}/auth/poll?session={}", self.base_url, session_id);
        let resp = self.client.get(poll_url).send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(PollResult {
                status: "not_found".to_string(),
                token: String::new(),
                refresh_token: String::new(),
            });
        }
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Poll failed: {}", resp.status()));
        }
        let res: PollResult = resp.json().await?;
        Ok(res)
    }

    pub async fn get_me(&self) -> Result<AuthMeResponse> {
        let resp = self
            .authenticated_request::<()>(Method::GET, "/auth/me", None)
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Get me failed: {}", resp.status()));
        }
        let res: AuthMeResponse = resp.json().await?;
        Ok(res)
    }

    pub async fn e2e_enable(&self, salt: Option<&str>, validator: Option<&str>) -> Result<String> {
        let body = serde_json::json!({
            "salt": salt,
            "validator": validator
        });

        let resp = self
            .authenticated_request(Method::POST, "/auth/e2e/enable", Some(&body))
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Enable E2E failed: {}", resp.status()));
        }
        #[derive(Deserialize)]
        struct EnableRes {
            encryption_salt: String,
        }
        let res: EnableRes = resp.json().await?;
        Ok(res.encryption_salt)
    }

    pub async fn reset_remote(&self) -> Result<()> {
        let resp = self
            .authenticated_request::<()>(Method::POST, "/sync/reset", None)
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Reset remote failed: {}", resp.status()));
        }
        Ok(())
    }

    pub async fn get_checkout_url(&self) -> Result<String> {
        let resp = self
            .authenticated_request::<()>(Method::POST, "/billing/checkout", None)
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Failed to get checkout URL: {}", resp.status()));
        }
        #[derive(Deserialize)]
        struct UrlRes {
            url: String,
        }
        let res: UrlRes = resp.json().await?;
        Ok(res.url)
    }

    pub async fn get_portal_url(&self) -> Result<String> {
        let resp = self
            .authenticated_request::<()>(Method::POST, "/billing/portal", None)
            .await?;
        if resp.status() != StatusCode::OK {
            return Err(anyhow!("Failed to get portal URL: {}", resp.status()));
        }
        #[derive(Deserialize)]
        struct UrlRes {
            url: String,
        }
        let res: UrlRes = resp.json().await?;
        Ok(res.url)
    }
}

#[derive(Deserialize)]
pub struct AuthMeResponse {
    #[serde(rename = "id")]
    pub _id: String,
    pub plan: String,
    pub subscription_status: String,
    pub subscription_end_date: Option<String>,
    pub encryption_salt: Option<String>,
    pub encryption_validator: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginSession {
    pub session_id: String,
    pub url: String,
}

#[derive(Deserialize)]
pub struct PollResult {
    pub status: String,
    pub token: String,
    pub refresh_token: String,
}

#[derive(Deserialize)]
pub struct PullResult {
    pub changes: Vec<Note>,
    pub has_more: bool,
    pub next_cursor: String,
}

pub struct SyncManager {
    client: APIClient,
    repo: Repo,
    status_tx: mpsc::Sender<SyncStatus>,
    trigger_rx: mpsc::Receiver<()>,
    crypto_key: Arc<Mutex<Option<Zeroizing<[u8; 32]>>>>,
}

impl SyncManager {
    pub fn new(
        repo: Repo,
        status_tx: mpsc::Sender<SyncStatus>,
        trigger_rx: mpsc::Receiver<()>,
        crypto_key: Arc<Mutex<Option<Zeroizing<[u8; 32]>>>>,
    ) -> Self {
        Self {
            client: APIClient::new(),
            repo,
            status_tx,
            trigger_rx,
            crypto_key,
        }
    }

    pub async fn start(mut self) {
        crate::logger::log("SyncManager: Started");

        self.try_sync().await;

        loop {
            tokio::select! {
                msg = self.trigger_rx.recv() => {
                    if msg.is_none() {
                        break;
                    }
                    crate::logger::log("SyncManager: Manual trigger received");
                    self.try_sync().await;
                }
            }
        }
    }

    async fn try_sync(&self) {
        let token = config::get_token();
        if token.is_empty() {
            let _ = self.status_tx.send(SyncStatus::Offline).await;
            return;
        }

        // 1. Fetch Plan First
        let me = match self.client.get_me().await {
            Ok(me) => me,
            Err(e) => {
                crate::logger::log(&format!("SyncManager: Failed to fetch plan: {:?}", e));
                let _ = self.status_tx.send(SyncStatus::Error).await;
                return;
            }
        };

        crate::logger::log(&format!("SyncManager: User Plan = {}", me.plan));

        // 2. Handle Free Plan (Local Only)
        if me.plan.trim().eq_ignore_ascii_case("free") {
            if self.repo.get_salt().await.unwrap_or(None).is_some() {
                crate::logger::log("SyncManager: Detected Free plan but local E2E salt exists. Removing salt (Remote reset assumed).");
                let _ = self.repo.delete_salt().await;
                let _ = config::delete_passphrase();
                {
                    let mut guard = self.crypto_key.lock().unwrap();
                    *guard = None;
                }
            }

            crate::logger::log("SyncManager: Free plan active. Sync disabled (Local Only).");
            let _ = self.status_tx.send(SyncStatus::Offline).await;
            return;
        }

        // 3. Paid Plan - Enforce E2E
        let has_key = {
            let guard = self.crypto_key.lock().unwrap();
            guard.is_some()
        };

        match self.repo.get_salt().await {
            Ok(Some(_)) => {
                // Salt exists, proceed
            }
            Ok(None) => {
                // If remote has salt but local doesn't, we might need to sync it or wait for UI
                if let Some(salt) = me.encryption_salt {
                    crate::logger::log(
                        "SyncManager: Remote has salt but local missing. Setting local salt.",
                    );
                    let _ = self.repo.set_salt(&salt).await;
                } else {
                    crate::logger::log("SyncManager: No encryption salt found. Sync disabled (E2E Setup required).");
                }
                let _ = self.status_tx.send(SyncStatus::Offline).await;
                return;
            }
            Err(e) => {
                crate::logger::log(&format!("SyncManager: Failed to check salt: {:?}", e));
                let _ = self.status_tx.send(SyncStatus::Error).await;
                return;
            }
        }

        if !has_key {
            crate::logger::log("SyncManager: Encrypted but locked. Waiting for passphrase.");
            let _ = self.status_tx.send(SyncStatus::Offline).await;
            return;
        }

        crate::logger::log("SyncManager: try_sync starting (E2E Enforced)");
        let _ = self.status_tx.send(SyncStatus::Syncing).await;

        match self.do_sync(&me.plan).await {
            Ok(_) => {
                crate::logger::log("SyncManager: Sync finished successfully");
                let _ = self.status_tx.send(SyncStatus::Synced).await;
            }
            Err(e) => {
                crate::logger::log(&format!("Sync Error: {:?}", e));
                if e.to_string().contains("Payment Required") {
                    let _ = self.status_tx.send(SyncStatus::PaymentRequired).await;
                } else {
                    let _ = self.status_tx.send(SyncStatus::Error).await;
                }
            }
        }
    }

    async fn do_sync(&self, plan: &str) -> Result<()> {
        // We still attempt pull even if plan is free (server filters it)
        // But push will fail if not pro.
        self.pull().await.context("Pull failed")?;

        match self.push(plan).await {
            Ok(_) => Ok(()),
            Err(e) => {
                // Check if error is "Payment Required"
                if e.to_string().contains("Payment Required") {
                    return Err(anyhow!("Payment Required"));
                }
                Err(e).context("Push failed")
            }
        }
    }

    async fn pull(&self) -> Result<()> {
        let cursor = self.repo.get_cursor().await?;

        let server_time = self.client.check_sync().await?;

        if server_time <= cursor {
            return Ok(());
        }

        let mut current_cursor = cursor;
        let mut page_count = 0;
        const MAX_PAGES: usize = 100;

        let key_opt = {
            let key_guard = self.crypto_key.lock().unwrap();
            key_guard.as_ref().map(|k| k.clone())
        };

        loop {
            if page_count >= MAX_PAGES {
                break;
            }
            page_count += 1;

            let res = self.client.pull_changes(&current_cursor).await?;
            let original_count = res.changes.len();

            let mut decrypted_changes = Vec::new();
            for mut note in res.changes {
                if note.is_encrypted == 1 {
                    if let Some(key) = &key_opt {
                        match crypto::decrypt(&note.content, key) {
                            Ok(plaintext) => {
                                note.content = plaintext;
                                note.is_encrypted = 0; // Decrypted for local storage
                                decrypted_changes.push(note);
                            }
                            Err(e) => {
                                crate::logger::log(&format!(
                                    "Failed to decrypt note {}: {}",
                                    note.id, e
                                ));
                            }
                        }
                    }
                } else {
                    crate::logger::log(&format!(
                        "Ignoring non-encrypted note {} from server (Plaintext sync is deprecated)",
                        note.id
                    ));
                }
            }

            if !decrypted_changes.is_empty() {
                self.repo
                    .pull_upsert_notes(decrypted_changes, res.next_cursor.clone())
                    .await?;
            } else if original_count > 0 {
                self.repo.set_last_synced(&res.next_cursor).await?;
            }

            if res.next_cursor == current_cursor {
                break;
            }
            current_cursor = res.next_cursor;

            if !res.has_more {
                break;
            }
        }
        Ok(())
    }

    async fn push(&self, plan: &str) -> Result<()> {
        if plan == "free" {
            crate::logger::log("SyncManager: Sync (Write) is disabled for Free plan.");
            return Ok(());
        }

        let notes = self.repo.get_unsynced_notes().await?;

        crate::logger::log(&format!(
            "SyncManager: push found {} unsynced notes",
            notes.len()
        ));

        let key_opt = {
            let key_guard = self.crypto_key.lock().unwrap();
            key_guard.as_ref().map(|k| k.clone())
        };

        for n in notes {
            let current_note_opt = self.repo.get_note(n.id.clone()).await?;

            if let Some(mut latest_n) = current_note_opt {
                // ALWAYS encrypt before pushing in the new model
                if let Some(key) = &key_opt {
                    match crypto::encrypt(&latest_n.content, key) {
                        Ok(ciphertext) => {
                            latest_n.content = ciphertext;
                            latest_n.is_encrypted = 1;
                        }
                        Err(e) => {
                            crate::logger::log(&format!(
                                "Failed to encrypt note {}: {}",
                                latest_n.id, e
                            ));
                            continue;
                        }
                    }
                } else {
                    // This should theoretically be blocked by try_sync, but for safety:
                    crate::logger::log(&format!(
                        "Skipping push for note {}: Key not available",
                        latest_n.id
                    ));
                    continue;
                }

                self.client.push_note(&latest_n).await?;
                self.repo.mark_as_synced(latest_n.id.clone()).await?;
            }
        }
        Ok(())
    }
}
