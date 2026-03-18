use rproxy::{load_config, save_config, Proxy};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Edit,
    Insert,
    Command,
    ConfirmDelete,
    Help,
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

pub struct App {
    pub mode: Mode,
    pub proxies: Vec<Proxy>,
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

}

impl App {
    pub fn new(config_path: PathBuf) -> Self {
        let (proxies, message) = if config_path.exists() {
            match load_config(&config_path) {
                Ok(p) => {
                    let count = p.len();
                    (p, Some(format!("Loaded {} proxies from {}", count, config_path.display())))
                }
                Err(e) => (Vec::new(), Some(format!("Error loading config: {}", e))),
            }
        } else {
            (Vec::new(), Some(format!("New file: {}", config_path.display())))
        };

        App {
            mode: Mode::Normal,
            proxies,
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

        match save_config(&save_path, &self.proxies) {
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
