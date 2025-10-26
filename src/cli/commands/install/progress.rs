use crate::colors::*;
use std::io::{self, Write};

#[derive(Debug)]
pub(super) struct ProgressRenderer {
    last_len: usize,
    last_status: String,
}

impl ProgressRenderer {
    pub(super) fn new() -> Self {
        Self {
            last_len: 0,
            last_status: String::new(),
        }
    }

    pub(super) fn render(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.last_status = message.clone();
        let mut out = io::stdout();
        let pad = if self.last_len > message.len() {
            self.last_len - message.len()
        } else {
            0
        };
        write!(out, "\r{}{}", message, " ".repeat(pad)).ok();
        out.flush().ok();
        self.last_len = message.len();
    }

    pub(super) fn finish(&mut self) {
        println!();
        self.last_len = 0;
    }

    pub(super) fn clear_line(&mut self) {
        if self.last_len == 0 {
            return;
        }
        print!("\r{}\r", " ".repeat(self.last_len));
        io::stdout().flush().ok();
        self.last_status.clear();
        self.last_len = 0;
    }
}

pub(super) fn format_status(kind: &str, detail: &str) -> String {
    let (color, action) = match kind {
        "resolving" => (C_CYAN, "resolving"),
        "downloading" => (C_CYAN, "downloading"),
        "extracting" => (C_MAGENTA, "extracting"),
        "linking" => (C_GREEN, "linking"),
        "fast" => (C_GREEN, "fast"),
        _ => (C_DIM, kind),
    };
    format!("{C_GRAY}[pacm]{C_RESET} {color}{action}{C_RESET} {detail}")
}
