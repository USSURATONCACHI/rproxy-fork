use rproxy::{load_config, save_config, Config, Proxy, Settings};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Edit,
    Insert,
    Command,
    ConfirmDelete,
    Help,
    Settings,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditField {
    Bind,
    Remote,
    Protocol,
}

impl EditField {
    pub fn next(&self) -> Self {
        match self {
            EditField::Bind => EditField::Remote,
            EditField::Remote => EditField::Protocol,
            EditField::Protocol => EditField::Bind,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            EditField::Bind => EditField::Protocol,
            EditField::Remote => EditField::Bind,
            EditField::Protocol => EditField::Remote,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsField {
    MaxConnections,
    MaxClientTunnels,
    KeepaliveIdle,
    KeepaliveInterval,
}

impl SettingsField {
    pub fn next(&self) -> Self {
        match self {
            SettingsField::MaxConnections => SettingsField::MaxClientTunnels,
            SettingsField::MaxClientTunnels => SettingsField::KeepaliveIdle,
            SettingsField::KeepaliveIdle => SettingsField::KeepaliveInterval,
            SettingsField::KeepaliveInterval => SettingsField::MaxConnections,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            SettingsField::MaxConnections => SettingsField::KeepaliveInterval,
            SettingsField::MaxClientTunnels => SettingsField::MaxConnections,
            SettingsField::KeepaliveIdle => SettingsField::MaxClientTunnels,
            SettingsField::KeepaliveInterval => SettingsField::KeepaliveIdle,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            SettingsField::MaxConnections => "Max TCP Connections",
            SettingsField::MaxClientTunnels => "Max UDP Tunnels",
            SettingsField::KeepaliveIdle => "Keepalive Idle (s)",
            SettingsField::KeepaliveInterval => "Keepalive Interval (s)",
        }
    }
}

pub struct App {
    pub mode: Mode,
    pub proxies: Vec<Proxy>,
    pub settings: Settings,
    pub selected: usize,
    pub config_path: PathBuf,
    pub dirty: bool,
    pub message: Option<String>,
    pub should_quit: bool,

    // Edit/Insert state
    pub edit_field: EditField,
    pub edit_bind: String,
    pub edit_remote: String,
    pub edit_protocol: String,
    pub cursor_pos: usize,

    // Command mode
    pub command_buffer: String,

    // Settings edit state
    pub settings_field: SettingsField,
    pub settings_buffer: String,
    pub settings_cursor: usize,
}

impl App {
    pub fn new(config_path: PathBuf) -> Self {
        let (proxies, settings, message) = if config_path.exists() {
            match load_config(&config_path) {
                Ok(config) => {
                    let count = config.proxies.len();
                    (
                        config.proxies,
                        config.settings,
                        Some(format!("Loaded {} proxies from {}", count, config_path.display())),
                    )
                }
                Err(e) => (Vec::new(), Settings::default(), Some(format!("Error loading config: {}", e))),
            }
        } else {
            (Vec::new(), Settings::default(), Some(format!("New file: {}", config_path.display())))
        };

        App {
            mode: Mode::Normal,
            proxies,
            settings,
            selected: 0,
            config_path,
            dirty: false,
            message,
            should_quit: false,
            edit_field: EditField::Bind,
            edit_bind: String::new(),
            edit_remote: String::new(),
            edit_protocol: String::from("TCP"),
            cursor_pos: 0,
            command_buffer: String::new(),
            settings_field: SettingsField::MaxConnections,
            settings_buffer: String::new(),
            settings_cursor: 0,
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.proxies.is_empty() && self.selected < self.proxies.len() - 1 {
            self.selected += 1;
        }
    }

    pub fn move_to_top(&mut self) {
        self.selected = 0;
    }

    pub fn move_to_bottom(&mut self) {
        if !self.proxies.is_empty() {
            self.selected = self.proxies.len() - 1;
        }
    }

    pub fn enter_edit(&mut self) {
        if self.proxies.is_empty() {
            self.message = Some("No entries to edit. Press 'a' to add one.".into());
            return;
        }
        let proxy = &self.proxies[self.selected];
        self.edit_bind = proxy.bind.clone();
        self.edit_remote = proxy.remote.clone();
        self.edit_protocol = proxy.protocol.clone();
        self.edit_field = EditField::Bind;
        self.cursor_pos = self.edit_bind.len();
        self.mode = Mode::Edit;
    }

    pub fn enter_insert(&mut self) {
        self.edit_bind = String::new();
        self.edit_remote = String::new();
        self.edit_protocol = String::from("TCP");
        self.edit_field = EditField::Bind;
        self.cursor_pos = 0;
        self.mode = Mode::Insert;
    }

    pub fn current_edit_value(&self) -> &str {
        match self.edit_field {
            EditField::Bind => &self.edit_bind,
            EditField::Remote => &self.edit_remote,
            EditField::Protocol => &self.edit_protocol,
        }
    }

    pub fn current_edit_value_mut(&mut self) -> &mut String {
        match self.edit_field {
            EditField::Bind => &mut self.edit_bind,
            EditField::Remote => &mut self.edit_remote,
            EditField::Protocol => &mut self.edit_protocol,
        }
    }

    pub fn next_field(&mut self) {
        self.edit_field = self.edit_field.next();
        self.cursor_pos = self.current_edit_value().len();
    }

    pub fn prev_field(&mut self) {
        self.edit_field = self.edit_field.prev();
        self.cursor_pos = self.current_edit_value().len();
    }

    pub fn toggle_protocol(&mut self) {
        self.edit_protocol = if self.edit_protocol == "TCP" {
            "UDP".into()
        } else {
            "TCP".into()
        };
    }

    pub fn insert_char(&mut self, c: char) {
        if self.edit_field == EditField::Protocol {
            return;
        }
        let pos = self.cursor_pos;
        let value = self.current_edit_value_mut();
        if pos <= value.len() {
            value.insert(pos, c);
            self.cursor_pos = pos + 1;
        }
    }

    pub fn delete_char(&mut self) {
        if self.edit_field == EditField::Protocol {
            return;
        }
        let pos = self.cursor_pos;
        let value = self.current_edit_value_mut();
        if pos > 0 && !value.is_empty() {
            value.remove(pos - 1);
            self.cursor_pos = pos - 1;
        }
    }

    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn cursor_right(&mut self) {
        let len = self.current_edit_value().len();
        if self.cursor_pos < len {
            self.cursor_pos += 1;
        }
    }

    pub fn confirm_edit(&mut self) -> Result<(), String> {
        // Validate
        super::validate::validate_address(&self.edit_bind)?;
        super::validate::validate_address(&self.edit_remote)?;
        super::validate::validate_protocol(&self.edit_protocol)?;

        let protocol = self.edit_protocol.to_uppercase();
        let proxy = Proxy {
            bind: self.edit_bind.clone(),
            remote: self.edit_remote.clone(),
            protocol,
        };

        match self.mode {
            Mode::Edit => {
                self.proxies[self.selected] = proxy;
                self.message = Some("Entry updated.".into());
            }
            Mode::Insert => {
                self.proxies.push(proxy);
                self.selected = self.proxies.len() - 1;
                self.message = Some("Entry added.".into());
            }
            _ => {}
        }
        self.dirty = true;
        self.mode = Mode::Normal;
        Ok(())
    }

    pub fn request_delete(&mut self) {
        if self.proxies.is_empty() {
            self.message = Some("No entries to delete.".into());
            return;
        }
        self.mode = Mode::ConfirmDelete;
        self.message = Some(format!(
            "Delete proxy #{} ({})? y/n",
            self.selected + 1,
            self.proxies[self.selected].bind
        ));
    }

    pub fn confirm_delete(&mut self) {
        if !self.proxies.is_empty() {
            self.proxies.remove(self.selected);
            if self.selected >= self.proxies.len() && self.selected > 0 {
                self.selected -= 1;
            }
            self.dirty = true;
            self.message = Some("Entry deleted.".into());
        }
        self.mode = Mode::Normal;
    }

    pub fn cancel_delete(&mut self) {
        self.mode = Mode::Normal;
        self.message = None;
    }

    pub fn enter_command(&mut self) {
        self.command_buffer.clear();
        self.mode = Mode::Command;
    }

    // --- Settings mode ---

    pub fn enter_settings(&mut self) {
        self.settings_field = SettingsField::MaxConnections;
        self.settings_buffer = self.get_settings_value(&SettingsField::MaxConnections);
        self.settings_cursor = self.settings_buffer.len();
        self.mode = Mode::Settings;
    }

    fn get_settings_value(&self, field: &SettingsField) -> String {
        match field {
            SettingsField::MaxConnections => self.settings.max_connections.to_string(),
            SettingsField::MaxClientTunnels => self.settings.max_client_tunnels.to_string(),
            SettingsField::KeepaliveIdle => self.settings.keepalive_idle.to_string(),
            SettingsField::KeepaliveInterval => self.settings.keepalive_interval.to_string(),
        }
    }

    pub fn settings_next_field(&mut self) {
        self.apply_settings_buffer();
        self.settings_field = self.settings_field.next();
        self.settings_buffer = self.get_settings_value(&self.settings_field);
        self.settings_cursor = self.settings_buffer.len();
    }

    pub fn settings_prev_field(&mut self) {
        self.apply_settings_buffer();
        self.settings_field = self.settings_field.prev();
        self.settings_buffer = self.get_settings_value(&self.settings_field);
        self.settings_cursor = self.settings_buffer.len();
    }

    pub fn settings_insert_char(&mut self, c: char) {
        if !c.is_ascii_digit() {
            return;
        }
        let pos = self.settings_cursor;
        if pos <= self.settings_buffer.len() {
            self.settings_buffer.insert(pos, c);
            self.settings_cursor = pos + 1;
        }
    }

    pub fn settings_delete_char(&mut self) {
        let pos = self.settings_cursor;
        if pos > 0 && !self.settings_buffer.is_empty() {
            self.settings_buffer.remove(pos - 1);
            self.settings_cursor = pos - 1;
        }
    }

    pub fn settings_cursor_left(&mut self) {
        if self.settings_cursor > 0 {
            self.settings_cursor -= 1;
        }
    }

    pub fn settings_cursor_right(&mut self) {
        if self.settings_cursor < self.settings_buffer.len() {
            self.settings_cursor += 1;
        }
    }

    fn apply_settings_buffer(&mut self) {
        let val = &self.settings_buffer;
        match self.settings_field {
            SettingsField::MaxConnections => {
                if let Ok(v) = val.parse::<usize>() {
                    if v > 0 { self.settings.max_connections = v; }
                }
            }
            SettingsField::MaxClientTunnels => {
                if let Ok(v) = val.parse::<usize>() {
                    if v > 0 { self.settings.max_client_tunnels = v; }
                }
            }
            SettingsField::KeepaliveIdle => {
                if let Ok(v) = val.parse::<u64>() {
                    if v > 0 { self.settings.keepalive_idle = v; }
                }
            }
            SettingsField::KeepaliveInterval => {
                if let Ok(v) = val.parse::<u64>() {
                    if v > 0 { self.settings.keepalive_interval = v; }
                }
            }
        }
    }

    pub fn confirm_settings(&mut self) {
        self.apply_settings_buffer();
        self.dirty = true;
        self.message = Some("Settings updated.".into());
        self.mode = Mode::Normal;
    }

    pub fn cancel_settings(&mut self) {
        self.mode = Mode::Normal;
        self.message = None;
    }

    // --- Commands ---

    pub fn execute_command(&mut self) {
        let cmd = self.command_buffer.trim().to_string();
        self.mode = Mode::Normal;

        match cmd.as_str() {
            "w" => self.save(None),
            "q" => {
                if self.dirty {
                    self.message = Some("Unsaved changes! Use :wq to save and quit, or :q! to force quit.".into());
                } else {
                    self.should_quit = true;
                }
            }
            "q!" => {
                self.should_quit = true;
            }
            "wq" => {
                self.save(None);
                if self.message.as_ref().map_or(true, |m| !m.starts_with("Error")) {
                    self.should_quit = true;
                }
            }
            _ if cmd.starts_with("w ") => {
                let path = cmd[2..].trim().to_string();
                self.save(Some(path));
            }
            _ => {
                self.message = Some(format!("Unknown command: {}", cmd));
            }
        }
    }

    fn save(&mut self, path: Option<String>) {
        let save_path = path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.config_path.clone());

        let config = Config {
            settings: self.settings.clone(),
            proxies: self.proxies.clone(),
        };

        match save_config(&save_path, &config) {
            Ok(()) => {
                self.dirty = false;
                self.message = Some(format!("Saved to {}", save_path.display()));
            }
            Err(e) => {
                self.message = Some(format!("Error saving: {}", e));
            }
        }
    }
}
