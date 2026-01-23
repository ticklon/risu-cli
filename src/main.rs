use anyhow::Result;
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time;
use tui_textarea::{CursorMove, TextArea};
use zeroize::Zeroizing;

mod config;
mod crypto;
mod db;
mod logger;
mod markdown;
mod sync;

use crate::db::{Note, Repo};
use sync::{APIClient, SyncManager, SyncStatus};

#[derive(PartialEq, Debug)]
enum ActivePane {
    List,
    Editor,
    Login,
    DeleteConfirm,
    ClearConfirm,
    Search,
    StatusDialog,
    PassphraseInput,
    E2ESetup,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum PendingKey {
    None,
    D,
    Y,
    G,
}

#[derive(Debug)]
enum Message {
    Key(event::KeyEvent),
    Resize(u16, u16),
    Paste(String),
    SyncStatusUpdate(SyncStatus),
    Tick,
    PollingTick,
    SubscriptionCheck,
}

const RISU_LOGO: &str = r###"   RISU NOTE
██████╗ ██╗███████╗██╗   ██╗
██╔══██╗██║██╔════╝██║   ██║
██████╔╝██║███████╗██║   ██║
██╔══██╗██║╚════██║██║   ██║
██║  ██║██║███████║╚██████╔╝
╚═╝  ╚═╝╚═╝╚══════╝ ╚═════╝ "###;

struct Model<'a> {
    repo: Repo,
    notes: Vec<Note>,
    filtered_notes: Vec<Note>,
    list_state: ListState,
    textarea: TextArea<'a>,
    search_textarea: TextArea<'a>,
    passphrase_textarea: TextArea<'a>,
    passphrase_confirm_textarea: TextArea<'a>,
    clear_confirm_textarea: TextArea<'a>,
    active_pane: ActivePane,
    mode: Mode,
    pending_key: PendingKey,
    current_note_id: Option<String>,
    sync_status: SyncStatus,
    sync_trigger: mpsc::Sender<()>,
    status_rx: mpsc::Receiver<SyncStatus>,
    status_tx: mpsc::Sender<SyncStatus>,

    api_client: APIClient,
    login_session: Option<sync::LoginSession>,
    polling_login: bool,
    polling_subscription: bool,

    note_to_delete: Option<Note>,

    clipboard: Option<arboard::Clipboard>,

    saved_feedback_until: Option<Instant>,

    sync_start_time: Option<Instant>,
    spinner_index: usize,
    pending_sync_end: bool,

    show_preview: bool,
    preview_scroll: u16,

    visual_anchor_row: Option<usize>,

    config: config::AppConfig,
    token_source: Option<config::TokenSource>,
    user_email: Option<String>,
    user_plan: Option<String>,
    user_subscription_status: Option<String>,
    user_subscription_end_date: Option<String>,
    last_error: Option<String>,

    crypto_key: Arc<Mutex<Option<Zeroizing<[u8; 32]>>>>,
    e2e_status: String,
    is_loading: bool,

    status_list_state: ListState,
    e2e_setup_step: usize, // 0: Enter, 1: Confirm
}

async fn unlock_process(
    repo: Repo,
    api_client: APIClient,
    passphrase: String,
    crypto_key: Arc<Mutex<Option<Zeroizing<[u8; 32]>>>>,
) -> Result<bool> {
    if passphrase.is_empty() {
        return Ok(false);
    }

    if let Some(salt) = repo.get_salt().await? {
        let key = crypto::derive_key_async(passphrase, salt).await?;

        // Validate passphrase if a validator exists on the server
        match api_client.get_me().await {
            Ok(me) => {
                if let Some(validator) = me.encryption_validator {
                    match crypto::decrypt(&validator, &key) {
                        Ok(decrypted) if decrypted == "RISU-VALID" => {
                            crate::logger::log("Passphrase validated successfully.");
                        }
                        _ => {
                            crate::logger::log("Invalid passphrase: Validation failed.");
                            return Ok(false);
                        }
                    }
                }
            }
            Err(e) => {
                crate::logger::log(&format!(
                    "Warning: Could not fetch validator from server: {}",
                    e
                ));
            }
        }

        let mut guard = crypto_key.lock().unwrap();
        *guard = Some(key);
        drop(guard);

        return Ok(true);
    }
    Ok(false)
}

impl<'a> Model<'a> {
    async fn new(
        repo: Repo,
        sync_trigger: mpsc::Sender<()>,
        status_rx: mpsc::Receiver<SyncStatus>,
        status_tx: mpsc::Sender<SyncStatus>,
        config: config::AppConfig,
        crypto_key: Arc<Mutex<Option<Zeroizing<[u8; 32]>>>>,
    ) -> Result<Self> {
        let token_data = config::get_token_data();
        let initial_pane = ActivePane::List;

        let user_email = if !token_data.id_token.is_empty() {
            config::get_user_email_from_token(&token_data.id_token).ok()
        } else {
            None
        };
        let token_source = Some(token_data.source);

        let clipboard = arboard::Clipboard::new().ok();

        let mut search_textarea = TextArea::default();
        search_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Search ")
                .border_style(Style::default().fg(config.theme.search_border)),
        );

        let mut passphrase_textarea = TextArea::default();
        passphrase_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Enter Passphrase ")
                .border_style(Style::default().fg(config.theme.border_active)),
        );
        passphrase_textarea.set_mask_char('•');

        let mut passphrase_confirm_textarea = TextArea::default();
        passphrase_confirm_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm Passphrase ")
                .border_style(Style::default().fg(config.theme.border_active)),
        );
        passphrase_confirm_textarea.set_mask_char('•');

        let mut clear_confirm_textarea = TextArea::default();
        clear_confirm_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm Clear (Type 'ClearAllData') ")
                .border_style(Style::default().fg(config.theme.sync_error)),
        );

        let mut model = Self {
            repo,
            notes: Vec::new(),
            filtered_notes: Vec::new(),
            list_state: ListState::default(),
            textarea: TextArea::default(),
            search_textarea,
            passphrase_textarea,
            passphrase_confirm_textarea,
            clear_confirm_textarea,
            active_pane: initial_pane,
            mode: Mode::Normal,
            pending_key: PendingKey::None,
            current_note_id: None,
            sync_status: SyncStatus::Offline,
            sync_trigger,
            status_rx,
            status_tx,
            api_client: APIClient::new(),
            login_session: None,
            polling_login: false,
            polling_subscription: false,
            note_to_delete: None,
            clipboard,
            saved_feedback_until: None,
            sync_start_time: None,
            spinner_index: 0,
            pending_sync_end: false,
            show_preview: false,
            preview_scroll: 0,
            visual_anchor_row: None,
            config,
            token_source,
            user_email,
            user_plan: None,
            user_subscription_status: None,
            user_subscription_end_date: None,
            last_error: None,
            crypto_key,
            e2e_status: "Disabled".to_string(),
            is_loading: false,
            status_list_state: ListState::default(),
            e2e_setup_step: 0,
        };
        model.refresh_notes(true).await?;
        model.setup_textarea();

        if model.repo.get_salt().await?.is_some() {
            model.e2e_status = "Locked".to_string();
            if let Ok(Some(pass)) = config::get_passphrase() {
                // Background unlock
                let repo = model.repo.clone();
                let client = APIClient::new();
                let key_store = model.crypto_key.clone();
                let tx = model.status_tx.clone();
                let pass_clone = pass.clone();

                tokio::spawn(async move {
                    let _ = tx.send(SyncStatus::Unlocking).await;
                    match unlock_process(repo, client, pass_clone, key_store).await {
                        Ok(true) => {
                            let _ = tx.send(SyncStatus::Unlocked).await;
                        }
                        Ok(false) => {
                            let _ = tx.send(SyncStatus::Error).await;
                        }
                        Err(e) => {
                            crate::logger::log(&format!("Unlock error: {}", e));
                            let _ = tx.send(SyncStatus::Error).await;
                        }
                    }
                });
            }
        }

        Ok(model)
    }

    fn setup_textarea(&mut self) {
        let theme = &self.config.theme;
        self.textarea
            .set_cursor_line_style(Style::default().bg(theme.editor_cursor_line));
        self.textarea
            .set_block(Block::default().borders(Borders::ALL).title(" Editor "));
    }

    fn setup_search_textarea(&mut self) {
        let theme = &self.config.theme;
        self.search_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Search ")
                .border_style(Style::default().fg(theme.search_border)),
        );
    }

    fn setup_passphrase_textarea_style(&mut self) {
        let theme = &self.config.theme;
        self.passphrase_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" New Passphrase ")
                .border_style(Style::default().fg(theme.border_active)),
        );
    }

    fn setup_confirm_textarea_style(&mut self) {
        let theme = &self.config.theme;
        self.passphrase_confirm_textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm Passphrase ")
                .border_style(Style::default().fg(theme.border_active)),
        );
    }

    async fn refresh_notes(&mut self, should_update_editor: bool) -> Result<()> {
        self.notes = self.repo.get_notes().await?;

        let query = self.search_textarea.lines()[0].to_lowercase();
        self.filtered_notes = if query.is_empty() {
            self.notes.clone()
        } else {
            self.notes
                .iter()
                .filter(|n| n.content.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };

        if self.filtered_notes.is_empty() {
            self.list_state.select(None);
        } else if self.list_state.selected().is_none()
            || self.list_state.selected().unwrap() >= self.filtered_notes.len()
        {
            self.list_state.select(Some(0));
        }

        if should_update_editor {
            self.update_editor_from_selection();
        }
        Ok(())
    }

    fn update_editor_from_selection(&mut self) {
        if let Some(note) = self
            .list_state
            .selected()
            .and_then(|i| self.filtered_notes.get(i))
        {
            if self.current_note_id.as_deref() != Some(&note.id) {
                self.textarea = TextArea::from(note.content.lines());
                self.current_note_id = Some(note.id.clone());
                self.preview_scroll = 0;
                self.setup_textarea();
            }
            return;
        }
        self.textarea = TextArea::default();
        self.current_note_id = None;
        self.setup_textarea();
    }

    async fn save_current_note(&mut self) -> Result<()> {
        let content = self.textarea.lines().join("\n");
        if content.trim().is_empty() {
            if let Some(id) = &self.current_note_id {
                self.repo.delete_note(id.clone()).await?;
                self.current_note_id = None;
                let _ = self.sync_trigger.try_send(());
            }
            self.refresh_notes(true).await?;
            return Ok(());
        }

        // Check for changes before saving
        if let Some(id) = &self.current_note_id {
            if let Some(original_note) = self.notes.iter().find(|n| &n.id == id) {
                if original_note.content == content {
                    return Ok(());
                }
            }
        }

        let is_e2e_enabled = self.e2e_status != "Disabled";
        let id = self
            .repo
            .save_note(self.current_note_id.clone(), content, is_e2e_enabled)
            .await?;
        self.current_note_id = Some(id);

        self.saved_feedback_until = Some(Instant::now() + Duration::from_secs(1));

        self.refresh_notes(true).await?;
        if !self.notes.is_empty() {
            self.list_state.select(Some(0));
            self.update_editor_from_selection();
        }

        let _ = self.sync_trigger.try_send(());
        Ok(())
    }

    async fn start_login(&mut self) -> Result<()> {
        let session = self.api_client.start_login_session().await?;
        open_browser(&session.url);
        self.login_session = Some(session);
        self.polling_login = true;
        Ok(())
    }

    async fn poll_login(&mut self) -> Result<bool> {
        if let Some(session) = &self.login_session {
            let res = self
                .api_client
                .poll_login_session(&session.session_id)
                .await?;
            if res.status == "success" {
                config::save_token_data(&res.token, &res.refresh_token)?;
                self.polling_login = false;
                self.login_session = None;
                self.user_email = config::get_user_email_from_token(&res.token).ok();

                self.is_loading = true;
                match self.api_client.get_me().await {
                    Ok(me) => {
                        self.user_plan = Some(me.plan.clone());
                        self.user_subscription_status = Some(me.subscription_status.clone());
                        self.user_subscription_end_date = me.subscription_end_date.clone();
                        let is_eligible = me.plan == "pro" || me.plan == "dev";
                        if is_eligible {
                            if let Some(salt) = me.encryption_salt {
                                self.repo.set_salt(&salt).await?;
                                self.e2e_status = "Locked".to_string();

                                let pass_opt = config::get_passphrase().unwrap_or(None);
                                if let Some(pass) = pass_opt {
                                    // Background unlock
                                    let repo = self.repo.clone();
                                    let client = APIClient::new();
                                    let key_store = self.crypto_key.clone();
                                    let tx = self.status_tx.clone();
                                    let pass_clone = pass.clone();

                                    tokio::spawn(async move {
                                        let _ = tx.send(SyncStatus::Unlocking).await;
                                        match unlock_process(repo, client, pass_clone, key_store)
                                            .await
                                        {
                                            Ok(true) => {
                                                let _ = tx.send(SyncStatus::Unlocked).await;
                                            }
                                            Ok(false) => {
                                                // This means passphrase exists but invalid for new account? Or just wrong.
                                                // UI should probably prompt.
                                                let _ = tx.send(SyncStatus::Error).await;
                                            }
                                            Err(_) => {
                                                let _ = tx.send(SyncStatus::Error).await;
                                            }
                                        }
                                    });
                                    // We don't wait here, but we default to List view.
                                    // If unlock fails, user will see Error status or "Locked".
                                    self.active_pane = ActivePane::List;
                                } else {
                                    self.active_pane = ActivePane::PassphraseInput;
                                    self.passphrase_textarea = TextArea::default();
                                    self.passphrase_textarea.set_mask_char('•');
                                    self.setup_passphrase_textarea_style();
                                }
                            } else {
                                // Eligible but no E2E setup -> Go to Setup
                                self.e2e_status = "Setup Required".to_string();
                                self.active_pane = ActivePane::E2ESetup;
                            }
                        } else {
                            self.e2e_status = "Disabled".to_string();
                            self.active_pane = ActivePane::List;
                            if self.repo.get_salt().await.unwrap_or(None).is_some() {
                                crate::logger::log("poll_login: Free plan detected but local salt exists. Cleaning up.");
                                let _ = self.repo.delete_salt().await;
                                let _ = config::delete_passphrase();
                                {
                                    let mut guard = self.crypto_key.lock().unwrap();
                                    *guard = None;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        crate::logger::log(&format!("Failed to get user info: {}", e));
                        self.active_pane = ActivePane::List;
                    }
                }
                self.is_loading = false;

                let _ = self.sync_trigger.send(()).await;
                return Ok(true);
            } else if res.status == "not_found" {
                self.polling_login = false;
                self.login_session = None;
                return Err(anyhow::anyhow!("Login session expired"));
            }
        }
        Ok(false)
    }

    async fn delete_note(&mut self) -> Result<()> {
        if let Some(note) = &self.note_to_delete {
            self.repo.delete_note(note.id.clone()).await?;
            self.refresh_notes(true).await?;
            let _ = self.sync_trigger.try_send(());
        }
        self.active_pane = ActivePane::List;
        self.note_to_delete = None;
        self.saved_feedback_until = None;
        Ok(())
    }

    async fn handle_key_event(&mut self, key: event::KeyEvent) -> Result<bool> {
        match self.active_pane {
            ActivePane::List => match key.code {
                KeyCode::Char('q') => return Ok(true),
                KeyCode::Esc => {
                    if !self.search_textarea.lines()[0].is_empty() {
                        self.search_textarea = TextArea::default();
                        self.setup_search_textarea();
                        self.refresh_notes(true).await?;
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => self.move_list_selection(1),
                KeyCode::Char('k') | KeyCode::Up => self.move_list_selection(-1),
                KeyCode::Char('r') => {
                    let _ = self.sync_trigger.try_send(());
                }
                KeyCode::Char('d') => {
                    if let Some(note) = self
                        .list_state
                        .selected()
                        .and_then(|i| self.filtered_notes.get(i))
                    {
                        self.note_to_delete = Some(note.clone());
                        self.active_pane = ActivePane::DeleteConfirm;
                    }
                }
                KeyCode::Enter | KeyCode::Tab => {
                    self.active_pane = ActivePane::Editor;
                    self.mode = Mode::Normal;
                }
                KeyCode::Char('g') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    self.active_pane = ActivePane::StatusDialog;
                    self.status_list_state.select(Some(0));
                }
                KeyCode::Char('i') => {
                    self.active_pane = ActivePane::Editor;
                    self.mode = Mode::Insert;
                    self.textarea.move_cursor(CursorMove::Bottom);
                    self.textarea.move_cursor(CursorMove::End);
                }
                KeyCode::Char('n') => {
                    self.current_note_id = None;
                    self.textarea = TextArea::default();
                    self.setup_textarea();
                    self.active_pane = ActivePane::Editor;
                    self.mode = Mode::Insert;
                }
                KeyCode::Char('/') => {
                    self.active_pane = ActivePane::Search;
                    self.setup_search_textarea();
                }
                KeyCode::Char('L') if self.e2e_status == "Locked" => {
                    self.active_pane = ActivePane::PassphraseInput;
                }
                _ => {}
            },
            ActivePane::Search => match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.active_pane = ActivePane::List;
                }
                _ => {
                    if self.search_textarea.input(key) {
                        self.refresh_notes(true).await?;
                    }
                }
            },
            ActivePane::StatusDialog => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.active_pane = ActivePane::List;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let items = self.get_status_menu_items();
                    let i = match self.status_list_state.selected() {
                        Some(i) => {
                            if i >= items.len() - 1 {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    self.status_list_state.select(Some(i));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    let items = self.get_status_menu_items();
                    let i = match self.status_list_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                items.len() - 1
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    self.status_list_state.select(Some(i));
                }
                KeyCode::Enter => {
                    if let Some(i) = self.status_list_state.selected() {
                        let items = self.get_status_menu_items();
                        if let Some(action) = items.get(i) {
                            match *action {
                                "Sync Now" => {
                                    let _ = self.sync_trigger.try_send(());
                                    self.active_pane = ActivePane::List;
                                }
                                "Login" => {
                                    let _ = self.start_login().await;
                                    self.active_pane = ActivePane::Login;
                                }
                                "Select Plan" => {
                                    if let Ok(url) = self.api_client.get_checkout_url().await {
                                        open_browser(&url);
                                    }
                                    self.active_pane = ActivePane::List;
                                    self.polling_subscription = true;
                                }
                                "Manage Subscription" => {
                                    if let Ok(url) = self.api_client.get_portal_url().await {
                                        open_browser(&url);
                                    }
                                    self.active_pane = ActivePane::List;
                                    self.polling_subscription = true;
                                }
                                "Logout" => {
                                    let _ = self.perform_logout().await;
                                    self.active_pane = ActivePane::List;
                                }
                                "Clear All Data" => {
                                    self.clear_confirm_textarea = TextArea::default();
                                    self.clear_confirm_textarea.set_block(
                                        Block::default()
                                            .borders(Borders::ALL)
                                            .title(" Confirm Clear (Type 'ClearAllData') ")
                                            .border_style(
                                                Style::default().fg(self.config.theme.sync_error),
                                            ),
                                    );
                                    self.active_pane = ActivePane::ClearConfirm;
                                }
                                "Close" => {
                                    self.active_pane = ActivePane::List;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            },
            ActivePane::ClearConfirm => match key.code {
                KeyCode::Esc => {
                    self.active_pane = ActivePane::StatusDialog;
                }
                KeyCode::Enter => {
                    let input = if self.clear_confirm_textarea.lines().is_empty() {
                        ""
                    } else {
                        self.clear_confirm_textarea.lines()[0].trim()
                    };

                    if input == "ClearAllData" {
                        self.perform_clear_all_data().await?;
                        self.active_pane = ActivePane::List;
                    } else {
                        self.active_pane = ActivePane::StatusDialog;
                    }
                }
                _ => {
                    self.clear_confirm_textarea.input(key);
                }
            },
            ActivePane::PassphraseInput => match key.code {
                KeyCode::Esc => {
                    self.active_pane = ActivePane::List;
                }
                KeyCode::Enter => {
                    let passphrase = self.passphrase_textarea.lines()[0].clone();
                    if !passphrase.is_empty() {
                        self.is_loading = true;

                        // Spawn unlock task
                        let repo = self.repo.clone();
                        let client = APIClient::new();
                        let key_store = self.crypto_key.clone();
                        let tx = self.status_tx.clone();
                        let pass_clone = passphrase.clone();

                        tokio::spawn(async move {
                            let _ = tx.send(SyncStatus::Unlocking).await;
                            match unlock_process(repo, client, pass_clone.clone(), key_store).await
                            {
                                Ok(true) => {
                                    let _ = config::save_passphrase(&pass_clone);
                                    let _ = tx.send(SyncStatus::Unlocked).await;
                                }
                                Ok(false) => {
                                    let _ = tx.send(SyncStatus::Error).await;
                                }
                                Err(_) => {
                                    let _ = tx.send(SyncStatus::Error).await;
                                }
                            }
                        });

                        self.passphrase_textarea = TextArea::default();
                        self.passphrase_textarea.set_mask_char('•');
                    }
                }
                _ => {
                    self.passphrase_textarea.input(key);
                }
            },
            ActivePane::E2ESetup => match key.code {
                KeyCode::Esc => {
                    self.active_pane = ActivePane::List;
                    self.e2e_setup_step = 0;
                    self.passphrase_textarea = TextArea::default();
                    self.passphrase_textarea.set_mask_char('•');
                    self.setup_passphrase_textarea_style(); // Helper to reset style
                    self.passphrase_confirm_textarea = TextArea::default();
                    self.passphrase_confirm_textarea.set_mask_char('•');
                    self.setup_confirm_textarea_style();
                }
                KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                    // Toggle focus
                    self.e2e_setup_step = 1 - self.e2e_setup_step;
                }
                KeyCode::Enter => {
                    let p1 = self.passphrase_textarea.lines()[0].clone();
                    let p2 = self.passphrase_confirm_textarea.lines()[0].clone();

                    if p1.is_empty() {
                        self.e2e_setup_step = 0;
                        return Ok(false);
                    }

                    if self.e2e_setup_step == 0 {
                        self.e2e_setup_step = 1;
                    } else {
                        // Submit
                        if p1 != p2 {
                            // Mismatch - reset confirm
                            self.passphrase_confirm_textarea = TextArea::default();
                            self.passphrase_confirm_textarea.set_mask_char('•');
                            self.setup_confirm_textarea_style();
                            crate::logger::log("Passphrases do not match");
                            return Ok(false);
                        }

                        self.is_loading = true;

                        // 1. Generate Salt locally
                        let salt = crypto::generate_salt();

                        // 2. Derive key and create Validator
                        match crypto::derive_key_async(p1.clone(), salt.clone()).await {
                            Ok(key) => {
                                match crypto::encrypt("RISU-VALID", &key) {
                                    Ok(validator) => {
                                        // 3. Send Salt + Validator atomically
                                        match self
                                            .api_client
                                            .e2e_enable(Some(&salt), Some(&validator))
                                            .await
                                        {
                                            Ok(_returned_salt) => {
                                                // Should match our salt
                                                self.repo.set_salt(&salt).await?;
                                                config::save_passphrase(&p1)?;
                                                self.repo.set_notes_encrypted_status(1).await?;

                                                // Unlock immediately
                                                let mut guard = self.crypto_key.lock().unwrap();
                                                *guard = Some(key); // Key is already derived
                                                drop(guard);

                                                self.e2e_status = "Unlocked".to_string();
                                                self.active_pane = ActivePane::List;
                                                let _ = self.sync_trigger.try_send(());
                                            }
                                            Err(e) => {
                                                crate::logger::log(&format!(
                                                    "Failed to enable E2E: {}",
                                                    e
                                                ));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        crate::logger::log(&format!(
                                            "Failed to encrypt validator: {}",
                                            e
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                crate::logger::log(&format!("Failed to derive key: {}", e));
                            }
                        }

                        self.is_loading = false;

                        // Cleanup textareas

                        self.passphrase_textarea = TextArea::default();
                        self.passphrase_textarea.set_mask_char('•');
                        self.setup_passphrase_textarea_style();
                        self.passphrase_confirm_textarea = TextArea::default();
                        self.passphrase_confirm_textarea.set_mask_char('•');
                        self.setup_confirm_textarea_style();
                        self.e2e_setup_step = 0;
                    }
                }
                _ => {
                    if self.e2e_setup_step == 0 {
                        self.passphrase_textarea.input(key);
                    } else {
                        self.passphrase_confirm_textarea.input(key);
                    }
                }
            },
            ActivePane::Editor => match self.mode {
                Mode::Normal => match key.code {
                    KeyCode::Esc => {
                        let _ = self.save_current_note().await;
                        self.active_pane = ActivePane::List;
                        self.pending_key = PendingKey::None;
                        self.show_preview = false;
                    }
                    KeyCode::Char('i') => {
                        self.mode = Mode::Insert;
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        self.textarea.move_cursor(CursorMove::Back);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if self.show_preview {
                            self.preview_scroll = self.preview_scroll.saturating_add(1);
                        } else {
                            self.textarea.move_cursor(CursorMove::Down);
                        }
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if self.show_preview {
                            self.preview_scroll = self.preview_scroll.saturating_sub(1);
                        } else {
                            self.textarea.move_cursor(CursorMove::Up);
                        }
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        self.textarea.move_cursor(CursorMove::Forward);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('w') => {
                        self.textarea.move_cursor(CursorMove::WordForward);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('b') => {
                        self.textarea.move_cursor(CursorMove::WordBack);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('e') => {
                        self.textarea.move_cursor(CursorMove::WordForward);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('0') => {
                        self.textarea.move_cursor(CursorMove::Head);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('$') => {
                        self.textarea.move_cursor(CursorMove::End);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('u') => {
                        self.textarea.undo();
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('r') => {
                        self.textarea.redo();
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('m') => {
                        self.show_preview = !self.show_preview;
                        self.preview_scroll = 0;
                        self.pending_key = PendingKey::None;
                    }

                    KeyCode::Char('g') => {
                        if self.pending_key == PendingKey::G {
                            self.textarea.move_cursor(CursorMove::Top);
                            self.pending_key = PendingKey::None;
                        } else {
                            self.pending_key = PendingKey::G;
                        }
                    }
                    KeyCode::Char('G') => {
                        self.textarea.move_cursor(CursorMove::Bottom);
                        self.pending_key = PendingKey::None;
                    }

                    KeyCode::Char('d') => {
                        if self.pending_key == PendingKey::D {
                            let (row, _) = self.textarea.cursor();
                            let line = self.textarea.lines()[row].clone();
                            self.copy_to_clipboard(&format!("{}\n", line));
                            self.textarea.move_cursor(CursorMove::Head);
                            self.textarea.delete_line_by_end();
                            if !self.textarea.delete_next_char() {
                                self.textarea.move_cursor(CursorMove::Back);
                                self.textarea.delete_next_char();
                            }
                            self.pending_key = PendingKey::None;
                        } else {
                            self.pending_key = PendingKey::D;
                        }
                    }

                    KeyCode::Char('y') => {
                        if self.pending_key == PendingKey::Y {
                            let (row, _) = self.textarea.cursor();
                            let line = self.textarea.lines()[row].clone();
                            self.copy_to_clipboard(&format!("{}\n", line));
                            self.pending_key = PendingKey::None;
                        } else {
                            self.pending_key = PendingKey::Y;
                        }
                    }

                    KeyCode::Char('p') => {
                        if let Some(text) = self.get_from_clipboard() {
                            self.textarea.insert_str(&text);
                        }
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('v') => {
                        self.mode = Mode::Visual;
                        self.textarea.start_selection();
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('V') => {
                        self.mode = Mode::VisualLine;
                        let (row, _) = self.textarea.cursor();
                        self.visual_anchor_row = Some(row);
                        self.textarea.move_cursor(CursorMove::Head);
                        self.textarea.start_selection();
                        self.textarea.move_cursor(CursorMove::End);
                        self.pending_key = PendingKey::None;
                    }

                    KeyCode::Char('s') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        let _ = self.save_current_note().await;
                        self.pending_key = PendingKey::None;
                    }
                    _ => {
                        self.pending_key = PendingKey::None;
                    }
                },
                Mode::Insert => match key.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Char('s') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        let _ = self.save_current_note().await;
                    }
                    _ => {
                        self.textarea.input(key);
                    }
                },
                Mode::Visual => match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.textarea.cancel_selection();
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('y') => {
                        self.textarea.copy();
                        let text = self.textarea.yank_text();
                        self.copy_to_clipboard(&text);
                        self.mode = Mode::Normal;
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('d') => {
                        self.textarea.cut();
                        let text = self.textarea.yank_text();
                        self.copy_to_clipboard(&text);
                        self.mode = Mode::Normal;
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('g') => {
                        if self.pending_key == PendingKey::G {
                            self.textarea.move_cursor(CursorMove::Top);
                            self.pending_key = PendingKey::None;
                        } else {
                            self.pending_key = PendingKey::G;
                        }
                    }
                    KeyCode::Char('G') => {
                        self.textarea.move_cursor(CursorMove::Bottom);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        self.textarea.move_cursor(CursorMove::Back);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.textarea.move_cursor(CursorMove::Down);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.textarea.move_cursor(CursorMove::Up);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        self.textarea.move_cursor(CursorMove::Forward);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('w') => {
                        self.textarea.move_cursor(CursorMove::WordForward);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('b') => {
                        self.textarea.move_cursor(CursorMove::WordBack);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('e') => {
                        self.textarea.move_cursor(CursorMove::WordForward);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('0') => {
                        self.textarea.move_cursor(CursorMove::Head);
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('$') => {
                        self.textarea.move_cursor(CursorMove::End);
                        self.pending_key = PendingKey::None;
                    }
                    _ => {
                        self.pending_key = PendingKey::None;
                    }
                },
                Mode::VisualLine => match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.textarea.cancel_selection();
                        self.visual_anchor_row = None;
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('y') => {
                        self.textarea.copy();
                        let text = self.textarea.yank_text();
                        self.copy_to_clipboard(&text);
                        self.mode = Mode::Normal;
                        self.visual_anchor_row = None;
                        self.pending_key = PendingKey::None;
                    }
                    KeyCode::Char('d') => {
                        self.textarea.cut();
                        let text = self.textarea.yank_text();
                        self.copy_to_clipboard(&text);
                        self.mode = Mode::Normal;
                        self.visual_anchor_row = None;
                        self.pending_key = PendingKey::None;
                    }
                    _ => {
                        match key.code {
                            KeyCode::Char('j') | KeyCode::Down => {
                                self.textarea.move_cursor(CursorMove::Down);
                                self.pending_key = PendingKey::None;
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                self.textarea.move_cursor(CursorMove::Up);
                                self.pending_key = PendingKey::None;
                            }
                            KeyCode::Char('g') => {
                                if self.pending_key == PendingKey::G {
                                    self.textarea.move_cursor(CursorMove::Top);
                                    self.pending_key = PendingKey::None;
                                } else {
                                    self.pending_key = PendingKey::G;
                                    return Ok(false);
                                }
                            }
                            KeyCode::Char('G') => {
                                self.textarea.move_cursor(CursorMove::Bottom);
                                self.pending_key = PendingKey::None;
                            }
                            _ => {
                                self.pending_key = PendingKey::None;
                            }
                        }

                        if let Some(anchor) = self.visual_anchor_row {
                            let (current_row, _) = self.textarea.cursor();
                            self.textarea.cancel_selection();

                            if current_row < anchor {
                                self.textarea
                                    .move_cursor(CursorMove::Jump(anchor as u16, 0));
                                self.textarea.move_cursor(CursorMove::End);
                                self.textarea.start_selection();
                                self.textarea
                                    .move_cursor(CursorMove::Jump(current_row as u16, 0));
                                self.textarea.move_cursor(CursorMove::Head);
                            } else {
                                self.textarea
                                    .move_cursor(CursorMove::Jump(anchor as u16, 0));
                                self.textarea.move_cursor(CursorMove::Head);
                                self.textarea.start_selection();
                                self.textarea
                                    .move_cursor(CursorMove::Jump(current_row as u16, 0));
                                self.textarea.move_cursor(CursorMove::End);
                            }
                        }
                    }
                },
            },
            ActivePane::Login => match key.code {
                KeyCode::Char('q') => return Ok(true),
                KeyCode::Esc => {
                    self.active_pane = ActivePane::List;
                }
                KeyCode::Enter => {
                    if !self.polling_login {
                        let _ = self.start_login().await;
                    }
                }
                _ => {}
            },
            ActivePane::DeleteConfirm => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let _ = self.delete_note().await;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.active_pane = ActivePane::List;
                    self.note_to_delete = None;
                }
                _ => {}
            },
        }
        Ok(false)
    }

    async fn perform_account_check(&mut self) -> Result<()> {
        if self.config.general.offline_mode || self.user_email.is_none() {
            return Ok(());
        }

        self.is_loading = true;
        match self.api_client.get_me().await {
            Ok(me) => {
                self.user_plan = Some(me.plan.clone());
                self.user_subscription_status = Some(me.subscription_status.clone());
                self.user_subscription_end_date = me.subscription_end_date.clone();
                let is_eligible = me.plan == "pro" || me.plan == "dev";
                if is_eligible {
                    if let Some(salt) = me.encryption_salt {
                        self.repo.set_salt(&salt).await?;

                        let is_unlocked = {
                            let guard = self.crypto_key.lock().unwrap();
                            guard.is_some()
                        };

                        if is_unlocked {
                            self.e2e_status = "Unlocked".to_string();
                            crate::logger::log("perform_account_check: E2E already unlocked");
                            let _ = self.sync_trigger.try_send(());
                        } else {
                            self.e2e_status = "Locked".to_string();
                            if let Ok(Some(pass)) = config::get_passphrase() {
                                // Background unlock
                                let repo = self.repo.clone();
                                let client = APIClient::new();
                                let key_store = self.crypto_key.clone();
                                let tx = self.status_tx.clone();
                                let pass_clone = pass.clone();

                                tokio::spawn(async move {
                                    let _ = tx.send(SyncStatus::Unlocking).await;
                                    match unlock_process(repo, client, pass_clone, key_store).await
                                    {
                                        Ok(true) => {
                                            let _ = tx.send(SyncStatus::Unlocked).await;
                                        }
                                        Ok(false) => {
                                            let _ = tx.send(SyncStatus::Error).await;
                                        }
                                        Err(_) => {
                                            let _ = tx.send(SyncStatus::Error).await;
                                        }
                                    }
                                });
                            } else {
                                self.active_pane = ActivePane::PassphraseInput;
                                self.passphrase_textarea = TextArea::default();
                                self.passphrase_textarea.set_mask_char('•');
                                self.setup_passphrase_textarea_style();
                            }
                        }
                    } else {
                        // Eligible but no salt -> Setup needed
                        self.e2e_status = "Setup Required".to_string();
                        self.active_pane = ActivePane::E2ESetup;
                    }
                } else {
                    self.e2e_status = "Disabled".to_string();
                    if self.repo.get_salt().await.unwrap_or(None).is_some() {
                        crate::logger::log("perform_account_check: Free plan detected but local salt exists. Cleaning up.");
                        let _ = self.repo.delete_salt().await;
                        let _ = config::delete_passphrase();
                        {
                            let mut guard = self.crypto_key.lock().unwrap();
                            *guard = None;
                        }
                    }
                }
            }
            Err(e) => {
                let msg = format!("perform_account_check: Failed to get user info: {}", e);
                crate::logger::log(&msg);
                self.last_error = Some(msg);
            }
        }
        self.is_loading = false;
        Ok(())
    }

    async fn update(&mut self, msg: Message) -> Result<bool> {
        match msg {
            Message::Key(key) => {
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    return self.handle_key_event(key).await;
                }
            }
            Message::Resize(_w, _h) => {}
            Message::Paste(text) => {
                if self.active_pane == ActivePane::Editor {
                    let text = text.replace('\r', "");
                    self.textarea.insert_str(text);
                }
            }
            Message::SyncStatusUpdate(status) => {
                if status == SyncStatus::Syncing {
                    self.sync_start_time = Some(Instant::now());
                    self.sync_status = status;
                    self.pending_sync_end = false;
                } else if status == SyncStatus::Synced {
                    let should_update_editor = self.active_pane != ActivePane::Editor;
                    self.refresh_notes(should_update_editor).await?;
                    self.pending_sync_end = true;
                } else if status == SyncStatus::Unlocking {
                    self.e2e_status = "Unlocking...".to_string();
                    self.sync_status = status;
                } else if status == SyncStatus::Unlocked {
                    self.e2e_status = "Unlocked".to_string();
                    self.sync_status = SyncStatus::Synced; // Or idle
                    self.is_loading = false;
                    self.pending_sync_end = true; // Show synced momentarily

                    // Trigger sync once unlocked
                    let _ = self.sync_trigger.try_send(());

                    // If we were on PassphraseInput, go to List
                    if self.active_pane == ActivePane::PassphraseInput {
                        self.active_pane = ActivePane::List;
                        self.last_error = None;
                    }
                } else if status == SyncStatus::Error {
                    self.sync_status = status;
                    self.is_loading = false;

                    if self.active_pane == ActivePane::PassphraseInput {
                        // Assume error means invalid passphrase here if we were inputting it
                        self.passphrase_textarea = TextArea::default();
                        self.passphrase_textarea.set_mask_char('•');
                        self.passphrase_textarea.set_block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Invalid Passphrase! Try Again ")
                                .border_style(Style::default().fg(self.config.theme.sync_error)),
                        );
                    }
                } else if status == SyncStatus::PaymentRequired {
                    self.sync_status = status;
                    self.is_loading = false;
                    self.e2e_status = "Upgrade Required".to_string();
                    // Auto-open status dialog to prompt upgrade?
                    self.active_pane = ActivePane::StatusDialog;
                    // Pre-select "Upgrade to Pro" if possible (simple hack: set selection index)
                    // But list items are dynamic. Just opening dialog is good enough.
                } else {
                    self.sync_status = status;
                    self.sync_start_time = None;
                    self.pending_sync_end = false;
                }
            }
            Message::Tick => {
                self.spinner_index = (self.spinner_index + 1) % 4;
            }
            Message::PollingTick => {
                if self.polling_login {
                    let _ = self.poll_login().await;
                }
            }
            Message::SubscriptionCheck => {
                if self.polling_subscription {
                    if let Ok(me) = self.api_client.get_me().await {
                        let new_plan = me.plan.clone();
                        let current_plan = self.user_plan.clone().unwrap_or("free".to_string());

                        let is_paid_now = new_plan == "pro" || new_plan == "dev";
                        let was_free = current_plan == "free";

                        if was_free && is_paid_now {
                            crate::logger::log("Subscription upgrade detected!");
                            self.polling_subscription = false;
                            let _ = self.perform_account_check().await;
                        }

                        // Always update local state
                        self.user_plan = Some(new_plan);
                        self.user_subscription_status = Some(me.subscription_status);
                    }
                }
            }
        }
        Ok(false)
    }

    async fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        let mut poll_interval = time::interval(Duration::from_secs(2));
        let mut spinner_interval = time::interval(Duration::from_millis(100));
        let mut sub_poll_interval = time::interval(Duration::from_secs(3));

        let _ = self.perform_account_check().await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let _input_handle = std::thread::spawn(move || {
            while let Ok(evt) = event::read() {
                if tx.send(evt).is_err() {
                    break;
                }
            }
        });

        let mut should_render = true;

        loop {
            if self.pending_sync_end {
                let can_show = if let Some(start) = self.sync_start_time {
                    start.elapsed() >= Duration::from_millis(700)
                } else {
                    true
                };

                if can_show {
                    self.sync_status = SyncStatus::Synced;
                    self.sync_start_time = None;
                    self.pending_sync_end = false;
                    should_render = true;
                }
            }

            if let Some(until) = self.saved_feedback_until {
                if Instant::now() >= until {
                    self.saved_feedback_until = None;
                    should_render = true;
                }
            }

            if should_render {
                terminal.draw(|f| self.ui(f))?;
                should_render = false;
            }

            let mut messages = Vec::new();
            tokio::select! {
                Some(event) = rx.recv() => {
                    let process_event = |e| match e {
                        Event::Key(key) => Some(Message::Key(key)),
                        Event::Resize(w, h) => Some(Message::Resize(w, h)),
                        Event::Paste(text) => Some(Message::Paste(text)),
                        _ => None,
                    };
                    if let Some(m) = process_event(event) {
                        messages.push(m);
                    }
                    while let Ok(e) = rx.try_recv() {
                        if let Some(m) = process_event(e) {
                            messages.push(m);
                        }
                    }
                }
                Some(status) = self.status_rx.recv() => messages.push(Message::SyncStatusUpdate(status)),
                _ = spinner_interval.tick() => messages.push(Message::Tick),
                _ = poll_interval.tick(), if self.polling_login => messages.push(Message::PollingTick),
                _ = sub_poll_interval.tick(), if self.polling_subscription => messages.push(Message::SubscriptionCheck),
            }

            for msg in messages {
                if self.update(msg).await? {
                    return Ok(());
                }
                should_render = true;
            }
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        if let Some(cb) = &mut self.clipboard {
            let _ = cb.set_text(text.to_string());
        }
    }

    fn get_from_clipboard(&mut self) -> Option<String> {
        self.clipboard.as_mut().and_then(|cb| cb.get_text().ok())
    }

    fn move_list_selection(&mut self, delta: i32) {
        self.saved_feedback_until = None;
        if self.filtered_notes.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                let next = i as i32 + delta;
                if next < 0 {
                    0
                } else if next >= self.filtered_notes.len() as i32 {
                    self.filtered_notes.len() - 1
                } else {
                    next as usize
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.update_editor_from_selection();
    }

    fn ui(&mut self, f: &mut Frame) {
        let theme = self.config.theme.clone();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(f.area());

        let mode_text = if self.config.general.offline_mode {
            "Offline Mode".to_string()
        } else {
            let token = config::get_token();
            if !token.is_empty() {
                match config::get_user_id_from_token(&token) {
                    Ok(uid) => format!("User: {}", uid),
                    Err(_) => "Session Invalid".to_string(),
                }
            } else {
                "Guest Mode (Local Only)".to_string()
            }
        };
        let header_content = format!("{}\n {} • {}", RISU_LOGO, config::APP_VERSION, mode_text);
        let header = Paragraph::new(header_content)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(theme.logo).add_modifier(Modifier::BOLD));
        f.render_widget(header, chunks[0]);

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(chunks[1]);

        let selected_index = self.list_state.selected();
        let items: Vec<ListItem> = self
            .filtered_notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let raw_title = n.content.lines().next().unwrap_or("No Content");
                let title = sanitize_title(raw_title);
                let is_selected = Some(i) == selected_index;

                let date_str = DateTime::parse_from_rfc3339(&n.updated_at)
                    .map(|dt| {
                        dt.with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|_| n.updated_at.clone());

                let date_line = if is_selected {
                    ratatui::text::Line::from(format!("    Updated: {}", date_str))
                } else {
                    ratatui::text::Line::from(ratatui::text::Span::styled(
                        format!("    Updated: {}", date_str),
                        Style::default().fg(Color::DarkGray),
                    ))
                };

                let lines = vec![
                    ratatui::text::Line::from(format!("   {}", title)),
                    date_line,
                ];

                ListItem::new(lines)
            })
            .collect();

        let query = self.search_textarea.lines()[0].clone();
        let list_title = if query.is_empty() {
            " Notes ".to_string()
        } else {
            let display_query = if query.len() > 15 {
                format!("{}..", &query[0..12])
            } else {
                query.clone()
            };
            format!(" Notes (Filter: \"{}\") ", display_query)
        };

        let mut list_block = Block::default().borders(Borders::ALL).title(list_title);
        if let ActivePane::List = self.active_pane {
            list_block = list_block.border_style(Style::default().fg(theme.border_active));
        } else if let ActivePane::Search = self.active_pane {
            list_block = list_block.border_style(Style::default().fg(theme.border_inactive));
        } else {
            list_block = list_block.border_style(Style::default().fg(theme.border_inactive));
        }

        let show_feedback = self
            .saved_feedback_until
            .is_some_and(|t| Instant::now() < t);
        let highlight_style = if show_feedback {
            Style::default()
                .bg(theme.sync_synced)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .bg(theme.selection_bg)
                .fg(theme.selection_fg)
                .add_modifier(Modifier::BOLD)
        };

        let list = List::new(items)
            .block(list_block)
            .highlight_style(highlight_style)
            .highlight_symbol(">>");

        f.render_stateful_widget(list, main_chunks[0], &mut self.list_state);

        if self.show_preview {
            let content = self.textarea.lines().join("\n");
            let markdown_text = markdown::parse_markdown(&content);
            let mut preview_block = Block::default()
                .borders(Borders::ALL)
                .title(" Preview (Markdown) ");
            if let ActivePane::Editor = self.active_pane {
                preview_block =
                    preview_block.border_style(Style::default().fg(theme.border_active));
            } else {
                preview_block =
                    preview_block.border_style(Style::default().fg(theme.border_inactive));
            }
            let paragraph = Paragraph::new(markdown_text)
                .block(preview_block)
                .wrap(Wrap { trim: false })
                .scroll((self.preview_scroll, 0));
            f.render_widget(paragraph, main_chunks[1]);
        } else {
            let mut editor_block = Block::default().borders(Borders::ALL);
            if let ActivePane::Editor = self.active_pane {
                let (color, title) = match self.mode {
                    Mode::Normal => (theme.mode_normal, " Editor (Normal) "),
                    Mode::Insert => (theme.mode_insert, " Editor (Insert) "),
                    Mode::Visual => (theme.mode_normal, " Editor (Visual) "),
                    Mode::VisualLine => (theme.mode_normal, " Editor (Visual Line) "),
                };
                editor_block = editor_block
                    .border_style(Style::default().fg(color))
                    .title(title);
            } else {
                editor_block = editor_block
                    .border_style(Style::default().fg(theme.border_inactive))
                    .title(" Editor ");
                // Hide cursor and disable cursor line highlight when not in editor pane
                self.textarea.set_cursor_style(Style::default());
                self.textarea.set_cursor_line_style(Style::default());
            }

            if let ActivePane::Editor = self.active_pane {
                // Restore cursor style and cursor line highlight when active
                self.textarea
                    .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
                self.textarea
                    .set_cursor_line_style(Style::default().bg(theme.editor_cursor_line));
            }

            self.textarea.set_block(editor_block);
            f.render_widget(&self.textarea, main_chunks[1]);
        }

        if self.active_pane == ActivePane::Login {
            self.render_login(f, chunks[1]);
        } else if self.active_pane == ActivePane::DeleteConfirm {
            self.render_delete_confirm(f, chunks[1]);
        } else if self.active_pane == ActivePane::Search {
            let area = centered_rect(60, 20, f.area());
            let area = ratatui::layout::Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 3,
            };
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(&self.search_textarea, area);
        } else if self.active_pane == ActivePane::StatusDialog {
            self.render_status_dialog(f, chunks[1]);
        } else if self.active_pane == ActivePane::PassphraseInput {
            self.render_passphrase_input(f, chunks[1]);
        } else if self.active_pane == ActivePane::E2ESetup {
            self.render_e2e_setup(f, chunks[1]);
        } else if self.active_pane == ActivePane::ClearConfirm {
            let area = centered_rect(60, 20, f.area());
            let area = ratatui::layout::Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 3,
            };
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(&self.clear_confirm_textarea, area);
        }

        let sync_color = if show_feedback {
            theme.sync_synced
        } else if self.config.general.offline_mode {
            theme.sync_offline
        } else {
            match self.sync_status {
                SyncStatus::Synced => theme.sync_synced,
                SyncStatus::Syncing => theme.sync_syncing,
                SyncStatus::Offline => theme.sync_offline,
                SyncStatus::Error => theme.sync_error,
                SyncStatus::PaymentRequired => theme.sync_payment_required,
                SyncStatus::Unlocking => theme.sync_syncing,
                SyncStatus::Unlocked => theme.sync_synced,
            }
        };

        let sync_indicator = if show_feedback {
            " Saved! ".to_string()
        } else if self.config.general.offline_mode {
            " Offline Mode ".to_string()
        } else if self.sync_status == SyncStatus::Syncing || self.is_loading {
            let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let s = spinner[self.spinner_index % spinner.len()];
            if self.is_loading {
                format!(" {} Loading... ", s)
            } else {
                format!(" {} Syncing... ", s)
            }
        } else {
            format!(" {} ", self.sync_status.as_str())
        };

        let mut help_text = match self.active_pane {
            ActivePane::List => {
                let query = self.search_textarea.lines()[0].clone();
                if query.is_empty() {
                    " j/k: Move  •  Enter: Open  •  i: Edit  •  n: New  •  d: Delete  •  r: Sync  •  Ctrl+g: Info  •  q: Quit ".to_string()
                } else {
                    " j/k: Move  •  Enter: Open  •  i: Edit  •  /: Filter  •  Esc: Clear Filter  •  q: Quit ".to_string()
                }
            },
            ActivePane::Editor => match self.mode {
                Mode::Normal => " i: Insert  •  v: Visual  •  V: V-Line  •  m: Preview  •  Esc: Back(Save)  •  Ctrl+S: Save \n dd: DelLine  •  yy: CopyLine  •  p: Paste ".to_string(),
                Mode::Insert => " Esc: Normal Mode  •  Ctrl+S: Save ".to_string(),
                Mode::Visual | Mode::VisualLine => " y: Yank  •  d: Delete  •  Esc: Normal Mode \n Move: h/j/k/l ".to_string(),
            },
            ActivePane::Login => " Enter: Login  •  Esc: Skip(Offline)  •  q: Quit ".to_string(),
            ActivePane::DeleteConfirm => " y: Confirm  •  n: Cancel ".to_string(),
            ActivePane::Search => " Enter/Esc: Close ".to_string(),
            ActivePane::StatusDialog => " Esc/Enter/q: Close ".to_string(),
            ActivePane::PassphraseInput => " Enter: Unlock  •  Esc: Cancel ".to_string(),
            ActivePane::E2ESetup => " Tab: Switch Field  •  Enter: Submit  •  Esc: Cancel ".to_string(),
            ActivePane::ClearConfirm => " Type 'ClearAllData' + Enter: Confirm  •  Esc: Cancel ".to_string(),
        };

        if self.pending_key != PendingKey::None {
            let pending_char = match self.pending_key {
                PendingKey::D => "d",
                PendingKey::Y => "y",
                PendingKey::G => "g",
                _ => "",
            };
            help_text = format!("(Pending: {}) {}", help_text, pending_char);
        }

        let footer_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(12), Constraint::Min(1)])
            .split(chunks[2]);

        f.render_widget(
            Paragraph::new(sync_indicator)
                .style(Style::default().fg(sync_color).add_modifier(Modifier::BOLD)),
            footer_chunks[0],
        );
        f.render_widget(
            Paragraph::new(help_text)
                .style(Style::default().fg(theme.border_inactive))
                .wrap(Wrap { trim: true }),
            footer_chunks[1],
        );
    }

    fn render_login(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let theme = &self.config.theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Authentication Required ")
            .border_style(Style::default().fg(theme.border_active));

        let text = if self.polling_login {
            "\n  Browser opened. Waiting for login...\n"
        } else {
            "\n  You need to login to sync your notes.\n\n  Press [Enter] to login with Google\n  Press [Esc] to start in Offline Mode\n"
        };

        let p = Paragraph::new(text)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center);

        let login_area = centered_rect(50, 30, area);
        f.render_widget(ratatui::widgets::Clear, login_area);
        f.render_widget(p, login_area);
    }

    fn render_delete_confirm(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let theme = &self.config.theme;
        let note_title = self
            .note_to_delete
            .as_ref()
            .map(|n| n.content.lines().next().unwrap_or("No Content"))
            .unwrap_or("");

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Delete Note? ")
            .border_style(Style::default().fg(theme.sync_error));

        let text = format!(
            "\n  Are you sure you want to delete this note?\n\n  \"{}\"\n\n  (y/n)",
            note_title
        );
        let p = Paragraph::new(text)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center);

        let confirm_area = centered_rect(40, 30, area);
        f.render_widget(ratatui::widgets::Clear, confirm_area);
        f.render_widget(p, confirm_area);
    }

    fn render_status_dialog(&mut self, f: &mut Frame, area: ratatui::layout::Rect) {
        let theme = &self.config.theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Risu System Status ")
            .border_style(Style::default().fg(theme.border_active));

        let token_source_str = self
            .token_source
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let account_str = self.user_email.as_deref().unwrap_or("Not Logged In");
        let plan_raw = self.user_plan.as_deref().unwrap_or("Unknown");
        let plan_str = match plan_raw {
            "dev" => "Early bird",
            "pro" => "Pro",
            _ => plan_raw,
        };
        let sub_status = self.user_subscription_status.as_deref().unwrap_or("None");
        let sub_end = self.user_subscription_end_date.as_deref().unwrap_or("N/A");

        let online_mode = if self.config.general.offline_mode {
            "Offline (Manual)".to_string()
        } else if self.user_email.is_none() {
            "Offline (Guest)".to_string()
        } else if self
            .user_plan
            .as_deref()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("free")
        {
            "Offline (Free Plan)".to_string()
        } else {
            "Online (Local-First)".to_string()
        };

        let e2e_display = match self.e2e_status.as_str() {
            "Unlocked" => "Active (Unlocked)".to_string(),
            "Locked" => "Inactive (Locked)".to_string(),
            _ => "Disabled".to_string(),
        };

        let error_str = self.last_error.as_deref().unwrap_or("None");

        let text = format!(
            "  Account:      {}\n  Plan:         {}\n  Sub Status:   {} ({})\n  Token Store:  {}\n  Network:      {}\n  E2E Encrypt:  {}\n\n  Last Error:   {}",
            account_str, plan_str, sub_status, sub_end, token_source_str, online_mode, e2e_display, error_str
        );

        let menu_items_list = self.get_status_menu_items();
        let menu_items_count = menu_items_list.len() as u16;

        // Dynamic Height Calculation
        // Info text is about 8-9 lines. Menu is variable.
        // We need at least: 9 (info) + menu_count + 2 (border) + 1 (spacing)
        let min_height = 10 + menu_items_count + 2;

        let available_height = area.height;
        let dialog_height = if available_height < min_height {
            available_height.saturating_sub(2).max(10)
        } else {
            let target = std::cmp::max(available_height * 50 / 100, min_height);
            std::cmp::min(target, available_height.saturating_sub(2))
        };

        // Vertical Centering
        let v_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length((available_height.saturating_sub(dialog_height)) / 2),
                Constraint::Length(dialog_height),
                Constraint::Min(0),
            ])
            .split(area);

        let dialog_area_v = v_layout[1];

        // Horizontal Centering (60% width)
        let h_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(60),
                Constraint::Percentage(20),
            ])
            .split(dialog_area_v);

        let dialog_area = h_layout[1];

        f.render_widget(ratatui::widgets::Clear, dialog_area);

        // Layout splitting: Top for Info, Bottom for Menu
        let inner_area = block.inner(dialog_area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(menu_items_count)])
            .split(inner_area);

        f.render_widget(block, dialog_area); // Render outer border

        // Info Paragraph
        let p = Paragraph::new(text).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(p, chunks[0]);

        // Menu List
        let menu_items: Vec<ListItem> = menu_items_list
            .iter()
            .map(|i| ListItem::new(format!("  {}", i)))
            .collect();

        let menu = List::new(menu_items)
            .highlight_style(Style::default().fg(Color::Black).bg(theme.selection_bg))
            .highlight_symbol("> ");

        f.render_stateful_widget(menu, chunks[1], &mut self.status_list_state);
    }

    fn render_passphrase_input(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let area = centered_rect(50, 20, area);
        let area = ratatui::layout::Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 3,
        };
        f.render_widget(ratatui::widgets::Clear, area);
        f.render_widget(&self.passphrase_textarea, area);
    }

    fn render_e2e_setup(&mut self, f: &mut Frame, area: ratatui::layout::Rect) {
        let area = centered_rect(60, 40, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Setup E2E Encryption ")
            .border_style(Style::default().fg(self.config.theme.border_active));

        f.render_widget(ratatui::widgets::Clear, area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Info text
                Constraint::Length(3), // Input 1
                Constraint::Length(1), // Spacer
                Constraint::Length(3), // Input 2
                Constraint::Min(1),
            ])
            .margin(2)
            .split(area);

        let info = Paragraph::new(
            "Set a passphrase to encrypt your notes.\nThis passphrase cannot be recovered if lost.",
        )
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(self.config.theme.foreground));
        f.render_widget(info, chunks[0]);

        // Highlight active input
        if self.e2e_setup_step == 0 {
            self.passphrase_textarea
                .set_style(Style::default().fg(Color::Yellow));
            self.passphrase_confirm_textarea
                .set_style(Style::default().fg(Color::DarkGray));
        } else {
            self.passphrase_textarea
                .set_style(Style::default().fg(Color::DarkGray));
            self.passphrase_confirm_textarea
                .set_style(Style::default().fg(Color::Yellow));
        }

        // Ensure styles are set correctly (borders)
        self.setup_passphrase_textarea_style();
        self.setup_confirm_textarea_style();

        f.render_widget(&self.passphrase_textarea, chunks[1]);
        f.render_widget(&self.passphrase_confirm_textarea, chunks[3]);
    }

    fn get_status_menu_items(&self) -> Vec<&str> {
        let mut items = vec!["Sync Now"];

        if self.user_email.is_some() {
            if self.user_plan.as_deref() == Some("pro") || self.user_plan.as_deref() == Some("dev")
            {
                items.push("Manage Subscription");
            } else if self.user_plan.as_deref() == Some("free") {
                items.push("Select Plan");
            }
            items.push("Logout");
        } else {
            items.push("Login");
        }

        items.push("Clear All Data");
        items.push("Close");
        items
    }

    async fn perform_clear_all_data(&mut self) -> Result<()> {
        let token = config::get_token();
        if !token.is_empty() {
            // Logged in: Try to clear remote first
            if let Err(e) = self.api_client.reset_remote().await {
                logger::log(&format!("Failed to clear remote data: {}", e));
            } else {
                logger::log("Remote data cleared successfully.");
            }
        }

        // Clear local data
        self.repo.clear_all_data().await?;
        self.refresh_notes(true).await?;

        logger::log("All data cleared.");
        Ok(())
    }

    async fn perform_logout(&mut self) -> Result<()> {
        let _ = config::delete_token_data();
        let _ = config::delete_passphrase();

        self.user_email = None;
        self.token_source = None;
        self.user_plan = None;
        self.e2e_status = "Disabled".to_string();
        self.sync_status = SyncStatus::Offline;

        // Clear cached keys
        {
            let mut guard = self.crypto_key.lock().unwrap();
            *guard = None;
        }

        // Clear sensitive UI fields
        self.passphrase_textarea = TextArea::default();
        self.passphrase_textarea.set_mask_char('•');
        self.setup_passphrase_textarea_style();
        self.passphrase_confirm_textarea = TextArea::default();
        self.passphrase_confirm_textarea.set_mask_char('•');
        self.setup_confirm_textarea_style();

        // Refresh notes as guest/offline user
        self.refresh_notes(true).await?;
        Ok(())
    }
}

fn sanitize_title(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    // Collapse multiple spaces
    let result = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if result.is_empty() {
        "No Content".to_string()
    } else {
        result
    }
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn open_browser(url: &str) {
    let _ = webbrowser::open(url);
}

async fn logout(_repo: Repo) -> Result<()> {
    if config::get_token().is_empty() {
        println!("Already logged out.");
        return Ok(());
    }

    // repo.clear_all_data().await?; // Phase 7: Keep local data, only discard keys
    let _ = config::delete_token_data();
    let _ = config::delete_passphrase(); // Delete E2E passphrase too
    println!("Logged out successfully. Local data preserved but access keys removed.");
    Ok(())
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    Ok(())
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the TUI application (default)
    Tui,
    /// Login to Risu Cloud
    Login,
    /// Logout from Risu Cloud
    Logout,
    /// Reset local database (Forces full re-sync)
    ResetLocal,
}

// ...

async fn handle_cli_login(repo: Repo) -> Result<()> {
    let client = APIClient::new();

    // Check if already logged in
    let token = config::get_token();
    if !token.is_empty() {
        if let Ok(me) = client.get_me().await {
            if let Ok(email) = config::get_user_email_from_token(&token) {
                println!("Already logged in as: {}", email);
                let display_plan = match me.plan.as_str() {
                    "dev" => "Early bird",
                    "pro" => "Pro",
                    _ => &me.plan,
                };
                println!("Plan: {} ({})", display_plan, me.subscription_status);

                // Ensure salt is synced even if already logged in
                if let Some(salt) = me.encryption_salt {
                    repo.set_salt(&salt).await?;
                    println!("Encryption salt synced.");
                }

                return Ok(());
            }
        }
    }

    println!("Starting login process...");
    match client.start_login_session().await {
        Ok(session) => {
            println!("Please open the following URL in your browser to login:");
            println!("{}", session.url);

            open_browser(&session.url);

            print!("Waiting for authentication... ");
            io::stdout().flush()?;

            let spinner = ['|', '/', '-', '\\'];
            let mut spinner_idx = 0;

            // Polling loop
            loop {
                match client.poll_login_session(&session.session_id).await {
                    Ok(res) => {
                        if res.status == "success" {
                            config::save_token_data(&res.token, &res.refresh_token)?;
                            println!("\nLogin successful!");
                            if let Ok(email) = config::get_user_email_from_token(&res.token) {
                                println!("Logged in as: {}", email);
                            }

                            // Fetch user info to sync salt
                            match client.get_me().await {
                                Ok(me) => {
                                    if let Some(salt) = me.encryption_salt {
                                        repo.set_salt(&salt).await?;
                                        println!("Account synced. Encryption enabled.");
                                    } else {
                                        println!("Account synced.");
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Warning: Failed to fetch account info: {}", e);
                                }
                            }

                            break;
                        } else if res.status == "not_found" {
                            eprintln!("\nLogin session expired. Please try again.");
                            break;
                        }
                    }
                    Err(_) => {
                        // Ignore polling errors (e.g. 404/decoding) while waiting
                    }
                }

                // Update spinner
                print!("\x08{}", spinner[spinner_idx]);
                io::stdout().flush()?;
                spinner_idx = (spinner_idx + 1) % spinner.len();

                time::sleep(Duration::from_millis(1000)).await; // Poll every 1s
            }
        }
        Err(e) => {
            eprintln!("Failed to start login session: {}", e);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    logger::init();
    let repo = Repo::new()?;

    let args = Args::parse();

    match args.command {
        Some(Commands::Login) => {
            return handle_cli_login(repo).await;
        }
        Some(Commands::Logout) => {
            return logout(repo).await;
        }
        Some(Commands::ResetLocal) => {
            repo.clear_all_data().await?;
            println!("Local database reset successfully.");
            println!("When you start Risu next time, it will perform a full sync from the server.");
            return Ok(());
        }
        None | Some(Commands::Tui) => {
            // Proceed to TUI
        }
    }

    let (sync_trigger_tx, sync_trigger_rx) = mpsc::channel(1);
    let (status_tx, status_rx) = mpsc::channel(10);
    let crypto_key = Arc::new(Mutex::new(None));
    let app_config = config::load_config();

    let sync_handle = if !app_config.general.offline_mode {
        let sync_repo = repo.clone();
        let sync_key = Arc::clone(&crypto_key);
        let sync_manager =
            SyncManager::new(sync_repo, status_tx.clone(), sync_trigger_rx, sync_key);
        Some(tokio::spawn(async move { sync_manager.start().await }))
    } else {
        None
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut model = Model::new(
        repo,
        sync_trigger_tx,
        status_rx,
        status_tx.clone(),
        app_config,
        crypto_key,
    )
    .await?;
    let model_result = model.run(&mut terminal).await;

    drop(model);
    if let Some(handle) = sync_handle {
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
    let _ = restore_terminal();
    if let Err(err) = model_result {
        eprintln!("Error: {:?}", err);
    }
    Ok(())
}
