//! Minimal stderr logger. Warnings always print; informational and debug
//! messages print only in verbose mode. Diagnostics go to stderr so stdout
//! stays reserved for machine-readable output.

pub struct Logger {
    verbose: bool,
}

impl Logger {
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    pub fn warn(&self, msg: &str) {
        eprintln!("warn: {msg}");
    }

    pub fn info(&self, msg: &str) {
        if self.verbose {
            eprintln!("info: {msg}");
        }
    }

    pub fn debug(&self, msg: &str) {
        if self.verbose {
            eprintln!("debug: {msg}");
        }
    }
}
