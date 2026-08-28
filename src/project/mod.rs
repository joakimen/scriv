//! What a project in the working directory is made of, and what to run in it.
//!
//! [`detect`] reads the files a directory holds and names the toolchains they
//! belong to; [`install`] and [`build`] turn those into commands; [`deps`]
//! reads the same files again for what they *declare*; [`report`] renders any
//! of it for a terminal.
//!
//! I/O-free throughout: [`crate::cmd::project`] collects the directory into a
//! [`Scan`] and runs what comes back, so every rule here is exercised without
//! a filesystem.

pub mod build;
pub mod deps;
pub mod detect;
pub mod edn;
pub mod install;
pub mod jsonc;
pub mod report;

pub use detect::{Detection, Scan, Toolchain};

/// One command to run, labelled with the name it is reported under and the
/// files that selected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What the step is called in a status line — the toolchain, or the task
    /// runner.
    pub name: &'static str,
    /// The files that selected it, as a user would look for them.
    pub evidence: String,
    pub program: String,
    pub args: Vec<String>,
}

impl Step {
    /// A step from string literals, which is how every step is written.
    fn new(name: &'static str, evidence: String, program: &str, args: &[&str]) -> Self {
        Self {
            name,
            evidence,
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
        }
    }

    /// The command as a user would type it, for a dry run and for verbose
    /// output. Not quoted for a shell: nothing here is passed to one.
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// Rewrite a step to run under `mise exec`, so tools the project pins are
/// resolved without the shell having been re-entered — which is what a `mise
/// install` in the same run leaves behind.
pub fn through_mise(step: Step) -> Step {
    let mut args = vec!["exec".to_string(), "--".to_string(), step.program];
    args.extend(step.args);

    Step {
        name: step.name,
        evidence: step.evidence,
        program: "mise".to_string(),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_with_no_arguments_is_just_its_program() {
        assert_eq!(
            Step::new("task", String::new(), "task", &[]).command_line(),
            "task"
        );
    }

    #[test]
    fn wrapping_in_mise_keeps_the_label_and_the_arguments() {
        let wrapped = through_mise(Step::new("bun", "package.json".into(), "bun", &["install"]));

        assert_eq!(wrapped.name, "bun");
        assert_eq!(wrapped.evidence, "package.json");
        assert_eq!(wrapped.command_line(), "mise exec -- bun install");
    }
}
