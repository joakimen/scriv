//! Shell integration codegen for `scriv init`.
//!
//! The interactive shell layer — select-and-`cd`, select-and-edit, key bindings
//! — exists only because a child process cannot change its parent shell's
//! directory or write to its command line. Rather than hand-maintain that glue
//! in the user's config, `scriv` emits it, so it stays in lockstep with the CLI
//! and ships completions too.
//!
//! *What* is emitted is [`crate::binding`]'s business, and configuration; this
//! module is only the part that says it in fish.

use anyhow::Result;
use clap::Command;
use clap_complete::Shell;

use crate::binding::{Action, Bound, Integration, Kind};
use crate::config::ShellConfig;

/// Render shell integration for `shell`, meant to be `source`d.
///
/// For fish this is a function per bound action, a function per alias, a
/// key-binding function and completions; for every other shell it is
/// completions only — the helpers all turn on writing to the shell's own
/// command line or directory, which is fish-specific until another shell's
/// emitter is written.
pub fn integration(shell: Shell, cmd: &mut Command, config: &ShellConfig) -> Result<String> {
    let mut out = String::new();

    if shell == Shell::Fish {
        out.push_str(&fish(&crate::binding::resolve(config)?));
        out.push_str("\n# --- completions ---\n");
    }

    let mut completions = Vec::new();
    clap_complete::generate(shell, cmd, cmd.get_name().to_string(), &mut completions);
    out.push_str(&String::from_utf8_lossy(&completions));

    Ok(out)
}

/// The fish half: the two helpers every binding leans on, a function per bound
/// action, a function per alias, and `scriv_key_bindings`.
fn fish(integration: &Integration) -> String {
    let mut out = String::from(FISH_PREAMBLE);

    for action in integration.bound_actions() {
        out.push('\n');
        out.push_str(&fish_function(&format!("scriv-{}", action.id), action));
    }

    for alias in &integration.aliases {
        out.push('\n');
        out.push_str(&fish_function(&alias.trigger, alias.action));
    }

    out.push('\n');
    out.push_str(FISH_BINDINGS_HEADER);
    for binding in &integration.bindings {
        out.push_str(&fish_bind(binding));
    }
    out.push_str("end\n");

    out
}

/// One fish function: the same body whether it was reached by a key or by the
/// name an alias gave it.
fn fish_function(name: &str, action: &Action) -> String {
    format!(
        "function {name} --description \"{}\"\n{}end\n",
        fish_quote(action.description),
        fish_body(action)
    )
}

/// What the function does, which is what [`Kind`] decides.
///
/// `$argv` on the plain form is what lets an alias take arguments — `fe -t`,
/// `i --dump`. A key press passes none, so the same body serves both.
fn fish_body(action: &Action) -> String {
    let scriv = format!("command scriv {}", action.args.join(" "));
    match action.kind {
        Kind::Run => format!("    {scriv} $argv\n"),
        Kind::Cd => {
            format!("    set -l dir ({scriv})\n    or return\n    test -n \"$dir\"; and cd $dir\n")
        }
        Kind::Line => fish_line(&scriv),
        Kind::LineOrUp => format!(
            "    if commandline --paging-mode; or test (commandline --line) -gt 1\n\
             \x20       commandline -f up-line\n\
             \x20       return\n\
             \x20   end\n{}",
            fish_line(&scriv)
        ),
    }
}

/// Read a selection back onto the command line.
///
/// NUL on both sides: a multi-line entry read back through `(...)` would be
/// split on its newlines and rejoined with spaces. The query is quoted for the
/// mirror of that — bare `(commandline)` is a list, and an empty one is *two*
/// empty strings, one argument too many.
fn fish_line(scriv: &str) -> String {
    format!(
        "    {scriv} --print0 --query \"$(commandline)\" | read --local --null selected\n\
         \x20   and commandline --replace -- $selected\n"
    )
}

/// One `bind` line.
///
/// Anything that prints goes through `scriv-run-as-command`, so fish owns the
/// output. The two that write to the command line print nothing and want
/// `repaint` instead — running them as a command would submit what they had
/// just put on the line.
fn fish_bind(binding: &Bound) -> String {
    let key = &binding.trigger;
    let name = format!("scriv-{}", binding.action.id);
    match binding.action.kind {
        Kind::Run | Kind::Cd => format!("    bind {key} \"scriv-run-as-command {name}\"\n"),
        Kind::Line | Kind::LineOrUp => {
            format!("    bind {key} \"{name}; commandline -f repaint\"\n")
        }
    }
}

/// Make text safe inside a fish double-quoted string: `$` would expand and `"`
/// would end it. A description is scriv's own, but `$EDITOR` is in one.
fn fish_quote(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

/// What every emitted file opens with: the two helpers a binding needs, which
/// are the same whatever is bound.
const FISH_PREAMBLE: &str = r#"# scriv shell integration — generated by `scriv init fish`.
# Source this from config.fish, e.g. with:
#   scriv init fish | source
#
# Which keys and names appear below is configuration, not code: `[shell.bindings]`
# and `[shell.aliases]` in your scriv config name the action each one runs, and
# `scriv config init` writes the defaults out commented.

# Run a command as though it had been typed at the prompt, putting back whatever
# was already on the command line afterwards.
#
# This is what a binding that prints output needs. `commandline -f repaint`
# redraws from where fish believes the cursor is, which after output is (prompt
# height - 1) rows too high, so a multi-line prompt eats the last lines of every
# message. Handing the command to fish instead leaves fish owning the output;
# the cost is a history entry, which is honest.
#
# The command line is stashed because `execute` submits whatever is on it.
function scriv-run-as-command --description "Run a command as if it had been typed"
    set -g __scriv_stashed_commandline (commandline)
    set -g __scriv_stashed_cursor (commandline --cursor)
    commandline --replace -- "$argv"
    commandline -f execute
end

function scriv-restore-command-line --on-event fish_postexec
    set -q __scriv_stashed_commandline; or return
    commandline --replace -- $__scriv_stashed_commandline
    commandline --cursor $__scriv_stashed_cursor
    set -e __scriv_stashed_commandline
    set -e __scriv_stashed_cursor
end

# A jump goes through fish's own `cd`, not `builtin cd`: the function is what
# records where you came from, so `prevd` and `cd -` know about it.
"#;

/// Bindings are wrapped in a function the user calls from their own
/// `fish_user_key_bindings` rather than bound at source time, so they compose
/// with fish's binding lifecycle. It is defined even when nothing is bound —
/// a config.fish that calls it should not start erroring because the table was
/// emptied.
const FISH_BINDINGS_HEADER: &str = r#"# Rebind by calling `bind` yourself after `scriv_key_bindings`, or by editing
# `[shell.bindings]`; the last binding for a key wins.
function scriv_key_bindings --description "Bind scriv selectors to keys"
"#;
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Arg;

    fn dummy() -> Command {
        Command::new("scriv").arg(Arg::new("verbose").long("verbose"))
    }

    /// The integration a user with no `[shell]` section gets.
    fn defaults() -> String {
        integration(Shell::Fish, &mut dummy(), &ShellConfig::default()).unwrap()
    }

    /// The integration for a hand-written table.
    fn configured(bindings: &[(&str, &str)], aliases: &[(&str, &str)]) -> String {
        let table = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        };
        let config = ShellConfig {
            bindings: Some(table(bindings)),
            aliases: Some(table(aliases)),
        };
        integration(Shell::Fish, &mut dummy(), &config).unwrap()
    }

    #[test]
    fn fish_emits_helper_functions() {
        let out = defaults();
        assert!(out.contains("function scriv-repo-cd"));
        assert!(out.contains("function scriv-worktree-cd"));
        assert!(out.contains("function scriv-run-as-command"));
        assert!(out.contains("function scriv-restore-command-line"));
        assert!(out.contains("function scriv-repo-open"));
        assert!(out.contains("function scriv-file-edit"));
        assert!(out.contains("function i "));
        assert!(out.contains("function b "));
        assert!(out.contains("function fe"));
        assert!(out.contains("function kl"));
        assert!(out.contains("function scriv-note-edit"));
        assert!(out.contains("function scriv-branch-checkout"));
        assert!(out.contains("function scriv-pr-checkout"));
        assert!(out.contains("function scriv-pr-open"));
        assert!(out.contains("function scriv-history-select"));
        assert!(out.contains("function scriv-history-up"));
        assert!(out.contains("function scriv_key_bindings"));
    }

    #[test]
    fn a_configured_table_is_what_gets_bound() {
        let out = configured(&[("f6", "repo-cd"), ("ctrl-b", "project-build")], &[]);

        assert!(
            out.contains("bind f6 \"scriv-run-as-command scriv-repo-cd\""),
            "{out}"
        );
        assert!(
            out.contains("bind ctrl-b \"scriv-run-as-command scriv-project-build\""),
            "{out}"
        );
        assert!(!out.contains("bind ctrl-o"), "a default survived the table");
        assert!(
            !out.contains("function scriv-worktree-cd"),
            "a function was defined for a key nobody bound",
        );
    }

    #[test]
    fn an_alias_takes_the_name_the_config_gives_it() {
        let out = configured(&[], &[("build", "project-build")]);

        assert!(out.contains("function build "), "{out}");
        assert!(out.contains("command scriv project build $argv"), "{out}");
        assert!(!out.contains("function b "), "the default name survived");
    }

    /// A config.fish that calls it should not start erroring because the table
    /// was emptied.
    #[test]
    fn the_binding_function_is_defined_even_when_nothing_is_bound() {
        let out = configured(&[], &[]);

        assert!(out.contains("function scriv_key_bindings"), "{out}");
        assert!(!out.contains("    bind "), "{out}");
    }

    #[test]
    fn an_action_nobody_defines_stops_the_emission_rather_than_thinning_it() {
        let config = ShellConfig {
            bindings: Some(
                [("ctrl-o".to_string(), "repo-jump".to_string())]
                    .into_iter()
                    .collect(),
            ),
            aliases: None,
        };
        let error = integration(Shell::Fish, &mut dummy(), &config).unwrap_err();

        assert!(error.to_string().contains("repo-jump"), "{error}");
    }

    /// The description is written once, plainly, and each shell escapes it: in
    /// a fish double-quoted string `$EDITOR` would expand to the user's editor.
    #[test]
    fn a_description_cannot_expand_inside_the_string_it_is_written_in() {
        assert_eq!(fish_quote("open it in $EDITOR"), "open it in \\$EDITOR");
        assert_eq!(fish_quote("say \"this\""), "say \\\"this\\\"");
    }

    #[test]
    fn fish_emits_completions_and_keys() {
        let out = defaults();
        assert!(out.contains("complete -c scriv"));
        assert!(out.contains("bind ctrl-o"));
        assert!(out.contains("bind ctrl-t"));
        assert!(out.contains("bind f1"));
        assert!(out.contains("bind f2"));
        assert!(out.contains("bind f3"));
        assert!(out.contains("bind f7"));
        assert!(out.contains("bind f10"));
        assert!(out.contains("bind ctrl-g"));
        assert!(out.contains("bind ctrl-r"));
        assert!(out.contains("bind up"));
    }

    /// A history entry can be a multi-line command. Read back through fish's
    /// ordinary `(...)` substitution it would be split on those newlines and
    /// rejoined with spaces — a command the user never ran, handed to them
    /// looking like one they did. NUL on both sides is what prevents it.
    #[test]
    fn history_travels_nul_terminated() {
        let out = defaults();
        assert!(out.contains("history sel --print0"), "{out}");
        assert!(out.contains("read --local --null selected"), "{out}");
    }

    /// The query must reach scriv as exactly one argument. Bare `(commandline)`
    /// is a *list*, and on an empty command line — by far the common way to
    /// reach this selector — fish expands it to two empty strings, one argument
    /// more than `history sel` accepts, so ctrl-r fails before the selector
    /// opens. Only fish's quoted `"$(...)"` form is one argument every time.
    #[test]
    fn the_query_reaches_scriv_as_one_argument() {
        let out = defaults();
        assert!(out.contains(r#"--query "$(commandline)""#), "{out}");
    }

    #[test]
    fn up_falls_through_to_fish_where_the_selector_would_be_wrong() {
        let out = defaults();
        assert!(out.contains("commandline --paging-mode"), "{out}");
        assert!(out.contains("commandline --line) -gt 1"), "{out}");
    }

    /// The fallthrough has to name an *input* function: `up-or-search` is a
    /// shell function in fish 4, which `commandline -f` rejects silently.
    #[test]
    fn up_hands_back_to_fishs_own_input_function() {
        let out = defaults();
        assert!(out.contains("commandline -f up-line"), "{out}");
        assert!(
            !out.contains("-f up-or-search"),
            "up-or-search is not an input function in fish 4",
        );
    }

    #[test]
    fn fish_leaves_alt_alone() {
        let out = defaults();
        let alt: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("bind alt-"))
            .collect();
        assert!(alt.is_empty(), "scriv binds alt keys: {alt:?}");
    }

    /// ctrl-i *is* tab, ctrl-j newline and ctrl-m enter on terminals that do not
    /// speak a modern key encoding.
    #[test]
    fn fish_avoids_keys_that_collide_with_tab_and_enter() {
        let out = defaults();
        for key in ["bind ctrl-i", "bind ctrl-j", "bind ctrl-m"] {
            assert!(!out.contains(key), "{key} collides with tab or enter");
        }
    }

    #[test]
    fn fe_forwards_arguments() {
        let out = defaults();
        assert!(out.contains("command scriv edit $argv"));
    }

    #[test]
    fn fish_defines_only_the_habitual_aliases_unprefixed() {
        let out = defaults();
        let unprefixed: Vec<&str> = out
            .lines()
            .filter_map(|line| line.strip_prefix("function "))
            .filter_map(|rest| rest.split_whitespace().next())
            .filter(|name| !name.starts_with("scriv"))
            .collect();
        assert_eq!(unprefixed, vec!["fe", "kl", "i", "b"]);
    }

    #[test]
    fn kl_sends_the_uncatchable_signal() {
        let out = defaults();
        assert!(
            out.contains("command scriv proc kill --force $argv"),
            "{out}"
        );
    }

    /// `commandline -f repaint` redraws from where fish believes the cursor is,
    /// overwriting (prompt height - 1) rows of whatever a binding printed.
    #[test]
    fn bindings_that_produce_output_run_through_the_command_line() {
        let out = defaults();
        for key in ["ctrl-o", "ctrl-t", "f1", "f2", "f3", "ctrl-g", "f7", "f10"] {
            let bind = binding_for(&out, key);
            assert!(
                bind.contains("scriv-run-as-command"),
                "{key} repaints over the output it produces: {bind}",
            );
        }
    }

    #[test]
    fn the_history_bindings_still_repaint() {
        let out = defaults();
        for key in ["ctrl-r", "up"] {
            let bind = binding_for(&out, key);
            assert!(
                bind.contains("commandline -f repaint"),
                "{key} does not repaint: {bind}",
            );
            assert!(
                !bind.contains("scriv-run-as-command"),
                "{key} would submit the command it just selected: {bind}",
            );
        }
    }

    /// `execute` submits whatever is on the command line.
    #[test]
    fn the_typed_command_line_is_stashed_before_executing_and_put_back_after() {
        let out = defaults();
        let stash = out
            .split("function scriv-run-as-command")
            .nth(1)
            .expect("no scriv-run-as-command");
        let stash = stash.split("\nend").next().unwrap();
        let saves = stash.find("set -g __scriv_stashed_commandline");
        let executes = stash.find("commandline -f execute");
        assert!(
            matches!((saves, executes), (Some(s), Some(e)) if s < e),
            "the command line is not saved before it is submitted:\n{stash}",
        );
        assert!(
            out.contains("--on-event fish_postexec"),
            "nothing puts the command line back",
        );
        assert!(
            out.contains("commandline --cursor $__scriv_stashed_cursor"),
            "the cursor is not put back where it was",
        );
    }

    /// `builtin cd` does not maintain `$dirprev`, which `prevd` and `cd -` walk.
    #[test]
    fn the_repo_jump_goes_through_fishs_cd_so_prevd_knows_about_it() {
        let out = defaults();
        // Code only: the comment above it names the builtin to explain why it
        // is not used, and must not be what this reads.
        let code: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert!(
            !code.iter().any(|l| l.contains("builtin cd")),
            "the selector's cd is invisible to prevd",
        );
        assert!(code.iter().any(|l| l.contains("and cd $dir")), "{out}");
    }

    fn binding_for<'a>(out: &'a str, key: &str) -> &'a str {
        out.lines()
            .map(str::trim)
            .find(|l| l.starts_with(&format!("bind {key} ")))
            .unwrap_or_else(|| panic!("no binding for {key}"))
    }

    /// `scriv edit` is reached by typing `fe`, so the one other key fish leaves
    /// free stays free for the user's own binding.
    #[test]
    fn fish_leaves_ctrl_q_free() {
        let out = defaults();
        assert!(!out.contains("bind ctrl-q"), "{out}");
    }

    #[test]
    fn fish_avoids_the_crowded_function_keys() {
        let out = defaults();
        assert!(!out.contains("bind f4"));
        assert!(!out.contains("bind f5"));
    }

    /// With `up` taken by the selector, these are what is left of walking
    /// history one entry at a time.
    #[test]
    fn fish_leaves_history_navigation_alone() {
        let out = defaults();
        assert!(!out.contains("bind ctrl-p"), "ctrl-p is up-line");
        assert!(!out.contains("bind ctrl-n"), "ctrl-n is down-line");
    }

    #[test]
    fn non_fish_emits_completions_only() {
        let out = integration(Shell::Bash, &mut dummy(), &ShellConfig::default()).unwrap();
        assert!(out.contains("scriv")); // bash completion script mentions the command
        assert!(!out.contains("function scriv-repo-cd"));
    }
}
