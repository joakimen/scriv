//! What building a project means, when nothing is known about it beforehand.
//!
//! A committed task runner is the answer wherever there is one: whoever wrote
//! the `Taskfile` already decided what building this repository involves, and
//! guessing past that would be guessing against them. Only a project with no
//! runner is built out of its toolchains.

use super::detect::{Detection, PythonMode, Toolchain};
use super::{Scan, Step};

/// A task runner committed at the project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Task,
    Make,
    Just,
}

impl Runner {
    /// The command that runs its default target — the one a bare `make` runs.
    fn program(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Make => "make",
            Self::Just => "just",
        }
    }
}

/// The files that name a runner, in the order one is looked for.
const RUNNER_FILES: &[(&str, Runner)] = &[
    ("Taskfile.yml", Runner::Task),
    ("Taskfile.yaml", Runner::Task),
    ("taskfile.yml", Runner::Task),
    ("taskfile.yaml", Runner::Task),
    ("Taskfile.dist.yml", Runner::Task),
    ("Taskfile.dist.yaml", Runner::Task),
    ("GNUmakefile", Runner::Make),
    ("Makefile", Runner::Make),
    ("makefile", Runner::Make),
    ("justfile", Runner::Just),
    ("Justfile", Runner::Just),
    (".justfile", Runner::Just),
];

/// What `scriv project build` will do in a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Build {
    /// The commands to run, in order. Empty when nothing here builds.
    Steps(Vec<Step>),
    /// Two or more runners are committed, named by the file each was found as.
    /// Which of them builds the project is the repository's business, not
    /// scriv's, so it says what it found and stops.
    Ambiguous(Vec<String>),
}

/// Decide how to build the scanned directory.
pub fn plan(scan: &Scan, detections: &[Detection]) -> Build {
    let runners = runners(scan);
    match runners.as_slice() {
        [] => Build::Steps(detections.iter().filter_map(|d| step(d, scan)).collect()),
        [(runner, file)] => Build::Steps(vec![Step::new(
            runner.program(),
            file.clone(),
            runner.program(),
            &[],
        )]),
        _ => Build::Ambiguous(runners.into_iter().map(|(_, file)| file).collect()),
    }
}

/// The distinct runners the project commits, each with the file it was found
/// as. A `Makefile` beside a `makefile` is one runner, not an argument.
fn runners(scan: &Scan) -> Vec<(Runner, String)> {
    let mut found: Vec<(Runner, String)> = Vec::new();

    for (file, runner) in RUNNER_FILES {
        if scan.has(file) && !found.iter().any(|(seen, _)| seen == runner) {
            found.push((*runner, (*file).to_string()));
        }
    }

    found
}

/// The command that builds one toolchain, where building it means anything.
/// A dependency manager is not a build: `uv sync` and `terraform init` produce
/// nothing a build was asked for.
fn step(detection: &Detection, scan: &Scan) -> Option<Step> {
    let name = detection.toolchain.name();
    let evidence = detection.evidence.clone();
    let step =
        |program: &str, args: &[&str]| Some(Step::new(name, evidence.clone(), program, args));

    match detection.toolchain {
        Toolchain::Rust => step("cargo", &["build"]),
        Toolchain::Go => step("go", &["build", "./..."]),
        // A `package.json` with no `build` script has nothing to run, and
        // `npm run build` on one is an error rather than a no-op.
        Toolchain::Node(_) if declares(scan.text("package.json"), "scripts", "build") => {
            step(detection.toolchain.name(), &["run", "build"])
        }
        Toolchain::Deno if declares(deno_config(scan), "tasks", "build") => {
            step("deno", &["task", "build"])
        }
        Toolchain::Maven { wrapper } => {
            step(if wrapper { "./mvnw" } else { "mvn" }, &["-B", "package"])
        }
        Toolchain::Gradle { wrapper } => {
            step(if wrapper { "./gradlew" } else { "gradle" }, &["build"])
        }
        Toolchain::Mise
        | Toolchain::Node(_)
        | Toolchain::Deno
        | Toolchain::Clojure
        | Toolchain::Babashka
        | Toolchain::Python(PythonMode::Project | PythonMode::Requirements)
        | Toolchain::Terraform => None,
    }
}

/// The Deno configuration file, which is where its tasks are declared.
/// `deno.lock` detects the project but declares nothing.
fn deno_config(scan: &Scan) -> Option<&str> {
    scan.text("deno.json").or_else(|| scan.text("deno.jsonc"))
}

/// Whether a JSON manifest declares `name` under `section` — npm's `scripts`,
/// Deno's `tasks`. A manifest that will not parse declares nothing.
fn declares(manifest: Option<&str>, section: &str, name: &str) -> bool {
    let Some(text) = manifest else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&super::jsonc::to_json(text)) else {
        return false;
    };
    value
        .get(section)
        .and_then(|section| section.get(name))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::super::detect::PackageManager;
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn scan(names: &[&str], contents: &[(&str, &str)]) -> Scan {
        Scan {
            paths: names.iter().map(|name| (*name).to_string()).collect(),
            contents: contents
                .iter()
                .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
                .collect(),
        }
    }

    fn detections(toolchains: &[Toolchain]) -> Vec<Detection> {
        toolchains
            .iter()
            .map(|&toolchain| Detection {
                toolchain,
                evidence: "evidence".to_string(),
            })
            .collect()
    }

    fn commands(build: &Build) -> Vec<String> {
        match build {
            Build::Steps(steps) => steps.iter().map(Step::command_line).collect(),
            Build::Ambiguous(files) => panic!("ambiguous: {files:?}"),
        }
    }

    #[test]
    fn a_committed_runner_is_the_whole_build() {
        for (file, expected) in [
            ("Taskfile.yml", "task"),
            ("Makefile", "make"),
            ("justfile", "just"),
        ] {
            let scan = scan(&[file, "Cargo.toml"], &[]);
            assert_eq!(
                commands(&plan(&scan, &detections(&[Toolchain::Rust]))),
                vec![expected],
                "{file}"
            );
        }
    }

    #[test]
    fn the_runner_step_names_the_file_it_was_found_as() {
        let scan = scan(&["GNUmakefile"], &[]);
        let Build::Steps(steps) = plan(&scan, &[]) else {
            panic!("expected a plan");
        };
        assert_eq!(steps[0].evidence, "GNUmakefile");
        assert_eq!(steps[0].name, "make");
    }

    #[test]
    fn two_runners_are_reported_rather_than_chosen_between() {
        let scan = scan(&["Taskfile.yml", "Makefile"], &[]);
        assert_eq!(
            plan(&scan, &[]),
            Build::Ambiguous(vec!["Taskfile.yml".to_string(), "Makefile".to_string()])
        );
    }

    #[test]
    fn two_spellings_of_one_runner_are_still_one_runner() {
        let scan = scan(&["Makefile", "GNUmakefile", "makefile"], &[]);
        assert_eq!(commands(&plan(&scan, &[])), vec!["make"]);
    }

    #[test]
    fn without_a_runner_each_toolchain_builds_itself_in_detection_order() {
        let scan = scan(&[], &[]);
        let detections = detections(&[
            Toolchain::Mise,
            Toolchain::Rust,
            Toolchain::Go,
            Toolchain::Maven { wrapper: true },
            Toolchain::Gradle { wrapper: false },
        ]);

        assert_eq!(
            commands(&plan(&scan, &detections)),
            vec![
                "cargo build",
                "go build ./...",
                "./mvnw -B package",
                "gradle build",
            ]
        );
    }

    #[test]
    fn a_node_project_builds_only_when_it_has_a_build_script() {
        let with = scan(
            &["package.json"],
            &[("package.json", r#"{"scripts": {"build": "tsc"}}"#)],
        );
        let without = scan(
            &["package.json"],
            &[("package.json", r#"{"scripts": {"test": "vitest"}}"#)],
        );
        let node = detections(&[Toolchain::Node(PackageManager::Bun)]);

        assert_eq!(commands(&plan(&with, &node)), vec!["bun run build"]);
        assert_eq!(commands(&plan(&without, &node)), Vec::<String>::new());
    }

    #[test]
    fn a_deno_project_builds_only_when_it_has_a_build_task() {
        let with = scan(
            &["deno.jsonc"],
            &[("deno.jsonc", "{\n// tasks\n\"tasks\": {\"build\": \"x\",}}")],
        );
        let without = scan(&["deno.json"], &[("deno.json", "{}")]);
        let deno = detections(&[Toolchain::Deno]);

        assert_eq!(commands(&plan(&with, &deno)), vec!["deno task build"]);
        assert_eq!(commands(&plan(&without, &deno)), Vec::<String>::new());
    }

    #[test]
    fn a_manifest_that_will_not_parse_declares_nothing() {
        assert!(!declares(Some("{ not json"), "scripts", "build"));
        assert!(!declares(None, "scripts", "build"));
    }

    #[test]
    fn a_project_nothing_knows_how_to_build_plans_nothing() {
        let scan = scan(&[], &[]);
        let detections = detections(&[
            Toolchain::Python(PythonMode::Project),
            Toolchain::Terraform,
            Toolchain::Clojure,
            Toolchain::Babashka,
        ]);

        assert_eq!(commands(&plan(&scan, &detections)), Vec::<String>::new());
    }

    #[test]
    fn an_empty_directory_plans_nothing() {
        assert_eq!(
            plan(
                &Scan {
                    paths: BTreeSet::new(),
                    contents: BTreeMap::new()
                },
                &[]
            ),
            Build::Steps(vec![])
        );
    }
}
