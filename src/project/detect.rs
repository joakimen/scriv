//! Which toolchains a project uses, derived from the files it holds.
//!
//! Only the project root is looked at: a directory deeper down is its own
//! project, and installing what it needs is a run in that directory.

use std::collections::{BTreeMap, BTreeSet};

/// Node package manager, chosen by the lockfile the project commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

/// Whether a Python project is described by a manifest or by a plain list of
/// pinned requirements — a different file to read and a different `uv`
/// subcommand to install it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonMode {
    Project,
    Requirements,
}

/// What detection and dependency listing read: the names of the files in the
/// project root, and the text of the ones whose contents decide something.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub paths: BTreeSet<String>,
    /// Manifest text, keyed as `paths` names the file.
    pub contents: BTreeMap<String, String>,
}

impl Scan {
    pub fn has(&self, name: &str) -> bool {
        self.paths.contains(name)
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        self.contents.get(name).map(String::as_str)
    }

    /// Every Terraform module in the root, joined. HCL is declaration order
    /// independent, and which of `main.tf` and `versions.tf` holds the
    /// provider block is a matter of taste.
    pub fn terraform(&self) -> String {
        self.paths
            .iter()
            .filter(|path| path.ends_with(".tf"))
            .filter_map(|path| self.text(path))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A toolchain that applies to a project, with the files that gave it away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub toolchain: Toolchain,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolchain {
    Mise,
    Rust,
    Go,
    Node(PackageManager),
    Deno,
    Clojure,
    Babashka,
    Maven { wrapper: bool },
    Gradle { wrapper: bool },
    Python(PythonMode),
    Terraform,
}

impl Toolchain {
    /// What a status line, a plan row and a dependency listing all call it.
    /// Node answers with its package manager, since that is the command that
    /// will run.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mise => "mise",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Node(PackageManager::Bun) => "bun",
            Self::Node(PackageManager::Pnpm) => "pnpm",
            Self::Node(PackageManager::Yarn) => "yarn",
            Self::Node(PackageManager::Npm) => "npm",
            Self::Deno => "deno",
            Self::Clojure => "clojure",
            Self::Babashka => "babashka",
            Self::Maven { .. } => "maven",
            Self::Gradle { .. } => "gradle",
            Self::Python(_) => "python",
            Self::Terraform => "terraform",
        }
    }
}

/// Files that make a directory a mise project, including the asdf-style
/// `.tool-versions` that mise also reads.
pub const MISE_CONFIGS: &[&str] = &[
    "mise.toml",
    ".mise.toml",
    "mise.local.toml",
    ".mise.local.toml",
    ".tool-versions",
    ".mise/config.toml",
    "mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
];

/// Gradle build scripts, in the order a build file is looked for.
pub const GRADLE_BUILD_FILES: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// Deno configuration, in the order one is looked for. `deno.lock` detects the
/// project but declares nothing.
pub const DENO_CONFIGS: &[&str] = &["deno.json", "deno.jsonc", "deno.lock"];

/// Every file whose text is read: what detection looks inside, and what a
/// dependency listing is parsed from. Terraform modules are found by their
/// extension instead, since a module can be called anything.
pub const MANIFESTS: &[&str] = &[
    "mise.toml",
    ".mise.toml",
    "mise.local.toml",
    ".mise.local.toml",
    ".tool-versions",
    ".mise/config.toml",
    "mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
    "Cargo.toml",
    "go.mod",
    "package.json",
    "deno.json",
    "deno.jsonc",
    "deps.edn",
    "bb.edn",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "pyproject.toml",
    "requirements.txt",
];

/// The toolchains the scanned project uses, in a fixed order so two runs in the
/// same directory report the same thing in the same places.
pub fn detect(scan: &Scan) -> Vec<Detection> {
    let mut found = Vec::new();
    let mut push = |toolchain, evidence: String| {
        found.push(Detection {
            toolchain,
            evidence,
        })
    };

    if let Some(config) = first_match(scan, MISE_CONFIGS) {
        push(Toolchain::Mise, config.to_string());
    }
    if scan.has("Cargo.toml") {
        push(Toolchain::Rust, "Cargo.toml".to_string());
    }
    if scan.has("go.mod") {
        push(Toolchain::Go, "go.mod".to_string());
    }
    if scan.has("package.json") {
        let (manager, lockfile) = package_manager(scan);
        push(Toolchain::Node(manager), joined("package.json", lockfile));
    }
    if let Some(config) = first_match(scan, DENO_CONFIGS) {
        push(Toolchain::Deno, config.to_string());
    }
    if scan.has("deps.edn") {
        push(Toolchain::Clojure, "deps.edn".to_string());
    }
    if scan.has("bb.edn") {
        push(Toolchain::Babashka, "bb.edn".to_string());
    }
    if scan.has("pom.xml") {
        let wrapper = scan.has("mvnw");
        push(
            Toolchain::Maven { wrapper },
            joined("pom.xml", wrapper.then_some("mvnw")),
        );
    }
    if let Some(build_file) = first_match(scan, GRADLE_BUILD_FILES) {
        let wrapper = scan.has("gradlew");
        push(
            Toolchain::Gradle { wrapper },
            joined(build_file, wrapper.then_some("gradlew")),
        );
    }
    if scan.has("uv.lock") {
        push(
            Toolchain::Python(PythonMode::Project),
            "uv.lock".to_string(),
        );
    } else if scan.has("pyproject.toml")
        && scan.text("pyproject.toml").is_some_and(declares_project)
    {
        push(
            Toolchain::Python(PythonMode::Project),
            "pyproject.toml".to_string(),
        );
    } else if scan.has("requirements.txt") {
        push(
            Toolchain::Python(PythonMode::Requirements),
            "requirements.txt".to_string(),
        );
    }
    if let Some(module) = scan.paths.iter().find(|path| path.ends_with(".tf")) {
        push(Toolchain::Terraform, module.clone());
    }

    found
}

/// Whether a `pyproject.toml` describes a Python package rather than only
/// configuring tools such as ruff. Without the `[project]` table there is
/// nothing for an installer to install.
fn declares_project(pyproject: &str) -> bool {
    pyproject
        .lines()
        .any(|line| table_header(line) == Some("project"))
}

fn table_header(line: &str) -> Option<&str> {
    let (header, _) = line.trim().strip_prefix('[')?.split_once(']')?;
    Some(header.trim())
}

/// The first of `names` present, so evidence names a file the user can open.
fn first_match(scan: &Scan, names: &[&'static str]) -> Option<&'static str> {
    names.iter().copied().find(|name| scan.has(name))
}

fn joined(first: &str, second: Option<&str>) -> String {
    match second {
        Some(second) => format!("{first} + {second}"),
        None => first.to_string(),
    }
}

/// The package manager the committed lockfile names, falling back to npm for a
/// project that has a `package.json` and no lockfile.
fn package_manager(scan: &Scan) -> (PackageManager, Option<&'static str>) {
    const LOCKFILES: &[(&str, PackageManager)] = &[
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("package-lock.json", PackageManager::Npm),
    ];

    LOCKFILES
        .iter()
        .find(|(lockfile, _)| scan.has(lockfile))
        .map_or((PackageManager::Npm, None), |(lockfile, manager)| {
            (*manager, Some(lockfile))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn scan(names: &[&str]) -> Scan {
        Scan {
            paths: names.iter().map(|name| (*name).to_string()).collect(),
            contents: BTreeMap::new(),
        }
    }

    fn toolchains(names: &[&str]) -> Vec<Toolchain> {
        detect(&scan(names))
            .into_iter()
            .map(|detection| detection.toolchain)
            .collect()
    }

    fn evidence(names: &[&str]) -> Vec<String> {
        detect(&scan(names))
            .into_iter()
            .map(|detection| detection.evidence)
            .collect()
    }

    fn python_toolchains(pyproject: &str, names: &[&str]) -> Vec<Toolchain> {
        let mut scan = scan(names);
        scan.contents
            .insert("pyproject.toml".to_string(), pyproject.to_string());
        detect(&scan)
            .into_iter()
            .map(|detection| detection.toolchain)
            .collect()
    }

    #[test]
    fn an_empty_directory_has_no_toolchains() {
        assert_eq!(toolchains(&[]), vec![]);
    }

    #[test]
    fn unrelated_files_are_ignored() {
        assert_eq!(toolchains(&["README.md", "LICENSE", ".gitignore"]), vec![]);
    }

    #[test]
    fn tool_versions_counts_as_a_mise_project() {
        assert_eq!(toolchains(&[".tool-versions"]), vec![Toolchain::Mise]);
    }

    #[test]
    fn a_nested_mise_config_counts_as_a_mise_project() {
        assert_eq!(
            toolchains(&[".config/mise/config.toml"]),
            vec![Toolchain::Mise]
        );
    }

    #[test]
    fn the_lockfile_selects_the_node_package_manager() {
        let cases = [
            (vec!["package.json", "bun.lock"], PackageManager::Bun),
            (vec!["package.json", "bun.lockb"], PackageManager::Bun),
            (vec!["package.json", "pnpm-lock.yaml"], PackageManager::Pnpm),
            (vec!["package.json", "yarn.lock"], PackageManager::Yarn),
            (
                vec!["package.json", "package-lock.json"],
                PackageManager::Npm,
            ),
            (vec!["package.json"], PackageManager::Npm),
        ];
        for (files, expected) in cases {
            assert_eq!(
                toolchains(&files),
                vec![Toolchain::Node(expected)],
                "{files:?}"
            );
        }
    }

    #[test]
    fn bun_wins_over_the_other_lockfiles() {
        let files = [
            "package.json",
            "bun.lock",
            "pnpm-lock.yaml",
            "yarn.lock",
            "package-lock.json",
        ];
        assert_eq!(
            toolchains(&files),
            vec![Toolchain::Node(PackageManager::Bun)]
        );
    }

    #[test]
    fn a_lockfile_alone_is_not_a_node_project() {
        assert_eq!(toolchains(&["package-lock.json"]), vec![]);
    }

    #[test]
    fn build_wrappers_are_reported_when_they_are_committed() {
        assert_eq!(
            toolchains(&["pom.xml", "mvnw"]),
            vec![Toolchain::Maven { wrapper: true }]
        );
        assert_eq!(
            toolchains(&["pom.xml"]),
            vec![Toolchain::Maven { wrapper: false }]
        );
        assert_eq!(
            toolchains(&["build.gradle.kts", "gradlew"]),
            vec![Toolchain::Gradle { wrapper: true }]
        );
    }

    #[test]
    fn a_python_project_wins_over_bare_requirements() {
        assert_eq!(
            python_toolchains(
                "[project]\nname = \"x\"",
                &["pyproject.toml", "requirements.txt"]
            ),
            vec![Toolchain::Python(PythonMode::Project)]
        );
        assert_eq!(
            toolchains(&["requirements.txt"]),
            vec![Toolchain::Python(PythonMode::Requirements)]
        );
    }

    #[test]
    fn a_pyproject_that_only_configures_tools_is_not_a_python_project() {
        assert_eq!(
            python_toolchains(
                "[tool.ruff]\ntarget-version = \"py313\"",
                &["pyproject.toml"]
            ),
            vec![]
        );
    }

    #[test]
    fn a_tool_only_pyproject_still_falls_back_to_requirements() {
        assert_eq!(
            python_toolchains("[tool.ruff]", &["pyproject.toml", "requirements.txt"]),
            vec![Toolchain::Python(PythonMode::Requirements)]
        );
    }

    #[test]
    fn a_lockfile_makes_a_python_project_without_reading_the_manifest() {
        assert_eq!(
            toolchains(&["pyproject.toml", "uv.lock"]),
            vec![Toolchain::Python(PythonMode::Project)]
        );
    }

    #[test]
    fn project_tables_are_recognised_around_comments_and_arrays() {
        assert!(declares_project("# comment\n[project]\nname = \"x\""));
        assert!(declares_project("  [project]  "));
        assert!(!declares_project("[project.optional-dependencies]"));
        assert!(!declares_project("[[project]]"));
        assert!(!declares_project("[tool.poetry]\nname = \"x\""));
        assert!(!declares_project(""));
    }

    #[test]
    fn any_terraform_file_makes_a_terraform_project() {
        assert_eq!(toolchains(&["main.tf"]), vec![Toolchain::Terraform]);
        assert_eq!(toolchains(&["notes.tfnotes"]), vec![]);
    }

    #[test]
    fn detection_order_is_stable_across_a_polyglot_project() {
        let files = [
            "main.tf",
            "package.json",
            "bun.lock",
            "Cargo.toml",
            "mise.toml",
            "go.mod",
            "deps.edn",
        ];
        assert_eq!(
            toolchains(&files),
            vec![
                Toolchain::Mise,
                Toolchain::Rust,
                Toolchain::Go,
                Toolchain::Node(PackageManager::Bun),
                Toolchain::Clojure,
                Toolchain::Terraform,
            ]
        );
    }

    #[test]
    fn evidence_names_the_files_that_matched() {
        assert_eq!(
            evidence(&["mise.toml", "package.json", "pnpm-lock.yaml", "main.tf"]),
            vec!["mise.toml", "package.json + pnpm-lock.yaml", "main.tf"]
        );
    }

    #[test]
    fn evidence_reports_a_lockless_node_project_by_its_manifest_alone() {
        assert_eq!(evidence(&["package.json"]), vec!["package.json"]);
    }

    #[test]
    fn evidence_includes_the_build_wrapper_when_it_will_be_used() {
        assert_eq!(evidence(&["pom.xml", "mvnw"]), vec!["pom.xml + mvnw"]);
        assert_eq!(evidence(&["pom.xml"]), vec!["pom.xml"]);
    }

    #[test]
    fn a_node_project_is_named_after_the_manager_that_will_run() {
        assert_eq!(Toolchain::Node(PackageManager::Bun).name(), "bun");
        assert_eq!(Toolchain::Node(PackageManager::Npm).name(), "npm");
    }

    #[test]
    fn terraform_modules_are_read_as_one_document() {
        let mut scan = scan(&["main.tf", "versions.tf", "README.md"]);
        scan.contents
            .insert("main.tf".to_string(), "resource {}".to_string());
        scan.contents
            .insert("versions.tf".to_string(), "terraform {}".to_string());
        scan.contents
            .insert("README.md".to_string(), "not hcl".to_string());

        assert_eq!(scan.terraform(), "resource {}\nterraform {}");
    }

    #[test]
    fn every_file_detection_looks_inside_is_one_the_scan_is_told_to_read() {
        for config in MISE_CONFIGS {
            assert!(MANIFESTS.contains(config), "{config} is never read");
        }
        for name in ["pyproject.toml", "requirements.txt", "Cargo.toml"] {
            assert!(MANIFESTS.contains(&name), "{name} is never read");
        }
    }
}
