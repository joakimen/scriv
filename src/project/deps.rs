//! What a project's manifests declare, as `scriv project deps --dump` lists it.
//!
//! A best-effort read rather than a resolver: it reports what is written in the
//! files a project commits, not what a package manager would work out from
//! them. Nothing here locks, resolves a range, or follows a transitive
//! dependency — that is the install this command is the alternative to.
//!
//! Within a group, dependencies are listed by name rather than in file order,
//! so the same project reads the same way whichever ecosystem it is written in.

use super::detect::{Detection, PythonMode, Toolchain};
use super::{Scan, edn, jsonc, report};
use crate::term::paint;

/// One dependency, as its manifest names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    /// Whatever the manifest says about which version — an exact version, a
    /// range, a git ref, a path. `None` where it says nothing.
    pub version: Option<String>,
}

impl Dependency {
    fn new(name: impl Into<String>, version: Option<String>) -> Self {
        Self {
            name: name.into(),
            version: version.filter(|version| !version.is_empty()),
        }
    }
}

/// The dependencies a manifest gives one role: npm's `devDependencies`,
/// Cargo's `[build-dependencies]`, a Maven scope, a Gradle configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub context: String,
    pub deps: Vec<Dependency>,
}

impl Group {
    fn new(context: impl Into<String>, mut deps: Vec<Dependency>) -> Self {
        deps.sort_by_key(|dep| dep.name.to_lowercase());
        Self {
            context: context.into(),
            deps,
        }
    }
}

/// One toolchain's dependencies, and the file they were read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub toolchain: &'static str,
    /// The file as a user would open it, or `*.tf` where several were read.
    pub source: String,
    pub groups: Vec<Group>,
}

/// Read every detected toolchain's manifest, in detection order. A toolchain
/// whose manifest could not be read is still listed, with nothing under it:
/// that it was detected and declares nothing is the answer to the question.
pub fn list(detections: &[Detection], scan: &Scan) -> Vec<Manifest> {
    detections
        .iter()
        .map(|detection| manifest(detection, scan))
        .collect()
}

fn manifest(detection: &Detection, scan: &Scan) -> Manifest {
    let toolchain = detection.toolchain.name();
    let read = |source: &str, parse: fn(&str) -> Vec<Group>| Manifest {
        toolchain,
        source: source.to_string(),
        groups: scan.text(source).map(parse).unwrap_or_default(),
    };

    match detection.toolchain {
        Toolchain::Mise => {
            let source = &detection.evidence;
            let parse = if source == ".tool-versions" {
                tool_versions
            } else {
                mise_toml
            };
            read(source, parse)
        }
        Toolchain::Rust => read("Cargo.toml", cargo),
        Toolchain::Go => read("go.mod", go_mod),
        Toolchain::Node(_) => read("package.json", package_json),
        Toolchain::Deno => match first_present(scan, &["deno.json", "deno.jsonc"]) {
            Some(source) => read(source, deno_json),
            None => empty(toolchain, "deno.lock"),
        },
        Toolchain::Clojure => read("deps.edn", deps_edn),
        Toolchain::Babashka => read("bb.edn", deps_edn),
        Toolchain::Maven { .. } => read("pom.xml", pom),
        Toolchain::Gradle { .. } => match first_present(scan, super::detect::GRADLE_BUILD_FILES) {
            Some(source) => read(source, gradle),
            None => empty(toolchain, "build.gradle"),
        },
        Toolchain::Python(PythonMode::Project) => read("pyproject.toml", pyproject),
        Toolchain::Python(PythonMode::Requirements) => read("requirements.txt", requirements),
        Toolchain::Terraform => Manifest {
            toolchain,
            source: "*.tf".to_string(),
            groups: terraform(&scan.terraform()),
        },
    }
}

fn empty(toolchain: &'static str, source: &str) -> Manifest {
    Manifest {
        toolchain,
        source: source.to_string(),
        groups: Vec::new(),
    }
}

fn first_present<'a>(scan: &Scan, names: &[&'a str]) -> Option<&'a str> {
    names.iter().copied().find(|name| scan.text(name).is_some())
}

/// Drop the groups that turned out to be empty, so a manifest with a
/// `[dev-dependencies]` header and nothing under it does not claim one.
fn nonempty(groups: Vec<Group>) -> Vec<Group> {
    groups.into_iter().filter(|g| !g.deps.is_empty()).collect()
}

// --- mise -------------------------------------------------------------------

/// `[tools]`, whose values are a version, a list of versions, or a table with
/// the version under a key.
fn mise_toml(text: &str) -> Vec<Group> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(tools) = doc.get("tools").and_then(toml::Value::as_table) else {
        return Vec::new();
    };

    let deps = tools
        .iter()
        .map(|(name, value)| Dependency::new(name, toml_version(value)))
        .collect();

    nonempty(vec![Group::new("tools", deps)])
}

/// asdf's format, which mise also reads: a tool and its versions per line.
fn tool_versions(text: &str) -> Vec<Group> {
    let deps = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, versions) = line.split_once(char::is_whitespace)?;
            Some(Dependency::new(
                name,
                Some(versions.split_whitespace().collect::<Vec<_>>().join(" ")),
            ))
        })
        .collect();

    nonempty(vec![Group::new("tools", deps)])
}

// --- Rust -------------------------------------------------------------------

fn cargo(text: &str) -> Vec<Group> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };

    let mut groups: Vec<Group> = ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .map(|section| Group::new(*section, toml_deps(doc.get(*section))))
        .collect();

    // A workspace root declares the versions its members inherit, and is often
    // the only place a version is written at all.
    if let Some(workspace) = doc.get("workspace").and_then(toml::Value::as_table) {
        groups.push(Group::new(
            "workspace.dependencies",
            toml_deps(workspace.get("dependencies")),
        ));
    }

    nonempty(groups)
}

/// A table of name to version, where a value may be the version itself or a
/// table saying where the dependency comes from instead.
fn toml_deps(section: Option<&toml::Value>) -> Vec<Dependency> {
    section
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(name, value)| Dependency::new(name, toml_version(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// What a manifest value says about the version, in the order the keys are
/// worth reporting: the version itself, then where it comes from instead.
fn toml_version(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(version) => Some(version.clone()),
        toml::Value::Array(versions) => Some(
            versions
                .iter()
                .filter_map(toml::Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        toml::Value::Table(table) => {
            for key in ["version", "tag", "branch", "rev", "git", "path", "local"] {
                if let Some(text) = table.get(key).and_then(toml::Value::as_str) {
                    return Some(text.to_string());
                }
            }
            table
                .get("workspace")
                .and_then(toml::Value::as_bool)
                .and_then(|inherited| inherited.then(|| "workspace".to_string()))
        }
        _ => None,
    }
}

// --- Go ---------------------------------------------------------------------

/// `require` lines, both the single-line form and the parenthesised block.
/// `// indirect` is what go itself calls a dependency it pulled in, so it is a
/// group rather than a note.
fn go_mod(text: &str) -> Vec<Group> {
    let mut direct = Vec::new();
    let mut indirect = Vec::new();
    let mut in_block = false;

    for line in text.lines() {
        let line = line.trim();
        if in_block {
            if line.starts_with(')') {
                in_block = false;
                continue;
            }
        } else if let Some(rest) = line.strip_prefix("require") {
            let rest = rest.trim();
            if rest == "(" {
                in_block = true;
                continue;
            }
            push_go_dep(rest, &mut direct, &mut indirect);
            continue;
        } else {
            continue;
        }
        push_go_dep(line, &mut direct, &mut indirect);
    }

    nonempty(vec![
        Group::new("require", direct),
        Group::new("indirect", indirect),
    ])
}

fn push_go_dep(line: &str, direct: &mut Vec<Dependency>, indirect: &mut Vec<Dependency>) {
    let (body, comment) = match line.split_once("//") {
        Some((body, comment)) => (body, comment),
        None => (line, ""),
    };
    let mut parts = body.split_whitespace();
    let Some(name) = parts.next() else {
        return;
    };
    let dep = Dependency::new(name, parts.next().map(str::to_string));

    if comment.contains("indirect") {
        indirect.push(dep);
    } else {
        direct.push(dep);
    }
}

// --- Node and Deno ----------------------------------------------------------

const NODE_SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

fn package_json(text: &str) -> Vec<Group> {
    let Some(doc) = json(text) else {
        return Vec::new();
    };

    nonempty(
        NODE_SECTIONS
            .iter()
            .map(|section| Group::new(*section, json_deps(&doc, section)))
            .collect(),
    )
}

/// Deno's import map, which is where a Deno project names what it depends on.
fn deno_json(text: &str) -> Vec<Group> {
    let Some(doc) = json(text) else {
        return Vec::new();
    };

    nonempty(vec![Group::new("imports", json_deps(&doc, "imports"))])
}

fn json(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&jsonc::to_json(text)).ok()
}

fn json_deps(doc: &serde_json::Value, section: &str) -> Vec<Dependency> {
    doc.get(section)
        .and_then(serde_json::Value::as_object)
        .map(|table| {
            table
                .iter()
                .map(|(name, value)| Dependency::new(name, value.as_str().map(str::to_string)))
                .collect()
        })
        .unwrap_or_default()
}

// --- Clojure ----------------------------------------------------------------

/// `:deps`, and the `:extra-deps` of each alias that has any. Everything else
/// an alias carries — `:extra-paths`, `:main-opts` — is not a dependency.
fn deps_edn(text: &str) -> Vec<Group> {
    let Some(doc) = edn::parse(text) else {
        return Vec::new();
    };
    let mut groups = vec![Group::new("deps", coordinates(doc.get(":deps")))];

    if let Some(aliases) = doc.get(":aliases") {
        for (name, alias) in aliases.entries() {
            let Some(name) = name.text() else { continue };
            groups.push(Group::new(
                format!("aliases {name}"),
                coordinates(alias.get(":extra-deps")),
            ));
        }
    }

    nonempty(groups)
}

/// A map of library name to coordinate, where the coordinate says which of the
/// several ways of naming a version this one uses.
fn coordinates(deps: Option<&edn::Value>) -> Vec<Dependency> {
    let Some(deps) = deps else {
        return Vec::new();
    };

    deps.entries()
        .iter()
        .filter_map(|(name, coord)| {
            let name = name.text()?;
            let version = [":mvn/version", ":git/tag", ":git/sha", ":local/root"]
                .iter()
                .find_map(|key| coord.get(key).and_then(edn::Value::text))
                .map(str::to_string);
            Some(Dependency::new(name, version))
        })
        .collect()
}

// --- Maven ------------------------------------------------------------------

/// Every `<dependency>` in the POM, grouped by its scope. What
/// `<dependencyManagement>` declares is a version other modules inherit rather
/// than a dependency of this one, so it is a group of its own.
///
/// `${property}` versions are resolved against `<properties>`, since a POM that
/// keeps every version there would otherwise list none.
fn pom(text: &str) -> Vec<Group> {
    let text = strip_xml_comments(text);
    let properties = pom_properties(&text);
    let managed = block(&text, "dependencyManagement");

    let mut groups: Vec<(String, Vec<Dependency>)> = Vec::new();
    let mut rest = text.as_str();

    while let Some((body, after)) = next_block(rest, "dependency") {
        let name = format!(
            "{}:{}",
            tag_text(body, "groupId").unwrap_or_default(),
            tag_text(body, "artifactId").unwrap_or_default()
        );
        let version = tag_text(body, "version").map(|v| resolve(v, &properties));
        let context = if managed.is_some_and(|managed| managed.contains(body)) {
            "managed".to_string()
        } else {
            tag_text(body, "scope").unwrap_or("compile").to_string()
        };

        match groups.iter_mut().find(|(seen, _)| *seen == context) {
            Some((_, deps)) => deps.push(Dependency::new(name, version)),
            None => groups.push((context, vec![Dependency::new(name, version)])),
        }
        rest = after;
    }

    nonempty(
        groups
            .into_iter()
            .map(|(context, deps)| Group::new(context, deps))
            .collect(),
    )
}

fn pom_properties(text: &str) -> Vec<(String, String)> {
    let Some(properties) = block(text, "properties") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut rest = properties;

    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>').map(|offset| start + offset) else {
            break;
        };
        let name = &rest[start + 1..end];
        if name.starts_with('/') {
            rest = &rest[end + 1..];
            continue;
        }
        if let Some(value) = tag_text(&rest[start..], name) {
            found.push((name.to_string(), value.to_string()));
        }
        rest = &rest[end + 1..];
    }

    found
}

/// Substitute `${name}` from `properties`, leaving an unknown one as written.
fn resolve(version: &str, properties: &[(String, String)]) -> String {
    let Some(name) = version
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return version.to_string();
    };

    properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| version.to_string())
}

fn strip_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The contents of the first `<name>` element, and what follows its close.
fn next_block<'a>(text: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some((&text[start..end], &text[end + close.len()..]))
}

fn block<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    next_block(text, name).map(|(body, _)| body)
}

/// The text of the first `<name>` child, trimmed. Elements in a POM hold text
/// or elements, never both, so this needs no more than the first match.
fn tag_text<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let body = block(text, name)?.trim();
    (!body.is_empty() && !body.contains('<')).then_some(body)
}

// --- Gradle -----------------------------------------------------------------

/// The quoted coordinates inside a `dependencies { }` block, grouped by the
/// configuration each was declared under.
///
/// A build script is a program, so this reads only what is written literally:
/// a dependency taken from a version catalog (`libs.something`) names no
/// version here and is not listed.
fn gradle(text: &str) -> Vec<Group> {
    let mut groups: Vec<(String, Vec<Dependency>)> = Vec::new();
    let mut depth: i32 = 0;
    let mut block_depth: Option<i32> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        let opens = trimmed.matches('{').count() as i32;
        let closes = trimmed.matches('}').count() as i32;

        if block_depth.is_none() && trimmed.starts_with("dependencies") && opens > 0 {
            block_depth = Some(depth);
        } else if let Some(started) = block_depth {
            if let Some((config, coordinate)) = gradle_dependency(trimmed) {
                match groups.iter_mut().find(|(seen, _)| *seen == config) {
                    Some((_, deps)) => deps.push(coordinate),
                    None => groups.push((config, vec![coordinate])),
                }
            }
            if depth + opens - closes <= started {
                block_depth = None;
            }
        }

        depth += opens - closes;
    }

    nonempty(
        groups
            .into_iter()
            .map(|(context, deps)| Group::new(context, deps))
            .collect(),
    )
}

/// `implementation("group:artifact:version")`, in either language's syntax.
fn gradle_dependency(line: &str) -> Option<(String, Dependency)> {
    let config: String = line
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if config.is_empty() {
        return None;
    }

    let rest = &line[config.len()..];
    let quote = rest.find(['"', '\''])?;
    let delimiter = rest.as_bytes()[quote] as char;
    let body = &rest[quote + 1..];
    let coordinate = &body[..body.find(delimiter)?];

    let mut parts = coordinate.splitn(3, ':');
    let group = parts.next()?;
    let artifact = parts.next()?;
    Some((
        config,
        Dependency::new(
            format!("{group}:{artifact}"),
            parts.next().map(str::to_string),
        ),
    ))
}

// --- Python -----------------------------------------------------------------

fn pyproject(text: &str) -> Vec<Group> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut groups = Vec::new();

    if let Some(project) = doc.get("project").and_then(toml::Value::as_table) {
        groups.push(Group::new(
            "dependencies",
            requirement_list(project.get("dependencies")),
        ));
        groups.extend(named_lists(
            project.get("optional-dependencies"),
            "optional-dependencies",
        ));
    }
    groups.extend(named_lists(
        doc.get("dependency-groups"),
        "dependency-groups",
    ));

    // Poetry keeps its own table, and a Poetry project's `[project]` is often
    // metadata alone.
    if let Some(poetry) = doc
        .get("tool")
        .and_then(|tool| tool.get("poetry"))
        .and_then(|poetry| poetry.get("dependencies"))
    {
        groups.push(Group::new(
            "tool.poetry.dependencies",
            toml_deps(Some(poetry)),
        ));
    }

    if let Some(build) = doc.get("build-system").and_then(toml::Value::as_table) {
        groups.push(Group::new(
            "build-system",
            requirement_list(build.get("requires")),
        ));
    }

    nonempty(groups)
}

/// A table of name to list of requirements — `optional-dependencies`, and PEP
/// 735's `dependency-groups`.
fn named_lists(section: Option<&toml::Value>, label: &str) -> Vec<Group> {
    section
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(name, list)| {
                    Group::new(format!("{label}.{name}"), requirement_list(Some(list)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn requirement_list(value: Option<&toml::Value>) -> Vec<Dependency> {
    value
        .and_then(toml::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(toml::Value::as_str)
                .map(requirement)
                .collect()
        })
        .unwrap_or_default()
}

/// A plain list of pinned requirements. Lines beginning with `-` are options
/// to pip — another file to read, an editable install — rather than packages.
fn requirements(text: &str) -> Vec<Group> {
    let deps = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty() && !line.starts_with('-'))
        .map(requirement)
        .collect();

    nonempty(vec![Group::new("requirements", deps)])
}

/// Split a PEP 508 requirement into the package and everything the line says
/// about which release of it: `httpx[http2]>=0.27,<1` is `httpx[http2]` at
/// `>=0.27,<1`.
fn requirement(spec: &str) -> Dependency {
    let spec = spec.trim();
    let end = spec
        .find(|c: char| c.is_whitespace() || "<>=!~;@(".contains(c))
        .unwrap_or(spec.len());
    Dependency::new(spec[..end].trim(), Some(spec[end..].trim().to_string()))
}

// --- Terraform --------------------------------------------------------------

/// The providers a module pins, and the Terraform version it asks for. Modules
/// are deliberately not listed: `source = "../vpc"` is a path, and a registry
/// module is one line of the same block a provider would be several of.
fn terraform(text: &str) -> Vec<Group> {
    let core = hcl_string(text, "required_version")
        .map(|constraint| vec![Dependency::new("terraform", Some(constraint))])
        .unwrap_or_default();

    let mut deps = Vec::new();
    let providers = hcl_body(text, "required_providers").unwrap_or_default();
    let mut rest = providers.as_str();

    while let Some((local, body, after)) = hcl_entry(rest) {
        let name = hcl_string(&body, "source").unwrap_or_else(|| local.clone());
        deps.push(Dependency::new(name, hcl_string(&body, "version")));
        rest = after;
    }

    nonempty(vec![
        Group::new("required_version", core),
        Group::new("required_providers", deps),
    ])
}

/// The `key = "value"` assignment `key` names, anywhere in `text`.
fn hcl_string(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        if let Some(value) = rest.strip_prefix('"').and_then(|v| v.split('"').next()) {
            return Some(value.to_string());
        }
    }
    None
}

/// The body of the `name { ... }` block, matched on braces so a nested block
/// does not end it early.
fn hcl_body(text: &str, name: &str) -> Option<String> {
    let start = text.find(name)? + name.len();
    let open = text[start..].find('{')? + start;
    let end = matching_brace(text, open)?;
    Some(text[open + 1..end].to_string())
}

/// The next `name = { ... }` entry: its name, its body, and what follows it.
/// Anchored on the brace rather than the name, so an assignment that opens no
/// block is stepped over instead of ending the walk.
fn hcl_entry(text: &str) -> Option<(String, String, &str)> {
    let open = text.find('{')?;
    let name = text[..open]
        .trim_end()
        .strip_suffix('=')?
        .lines()
        .last()?
        .trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let end = matching_brace(text, open)?;
    Some((
        name.to_string(),
        text[open + 1..end].to_string(),
        &text[end + 1..],
    ))
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (offset, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

// --- Rendering --------------------------------------------------------------

/// The widest a dependency name is allowed to push the version column. A go
/// module path or a Maven coordinate can be long enough to leave the versions
/// off the right of the terminal.
const NAME_LIMIT: usize = 44;

/// The listing `--dump` prints: each manifest, its groups, and the dependencies
/// under each. A manifest that declares nothing still gets its heading, since
/// "detected, and declares nothing" is an answer.
pub fn listing(manifests: &[Manifest], color: bool) -> Vec<String> {
    let mut lines = Vec::new();

    for (index, manifest) in manifests.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }
        lines.push(format!(
            "{}  {}",
            paint(manifest.toolchain, report::color(index), color),
            paint(&manifest.source, DIM, color)
        ));

        if manifest.groups.is_empty() {
            lines.push(paint("  nothing declared", DIM, color));
            continue;
        }

        let width = manifest
            .groups
            .iter()
            .flat_map(|group| &group.deps)
            .map(|dep| dep.name.chars().count())
            .max()
            .unwrap_or(0)
            .min(NAME_LIMIT);

        for group in &manifest.groups {
            lines.push(format!("  {}", paint(&group.context, CONTEXT, color)));
            for dep in &group.deps {
                let row = match &dep.version {
                    Some(version) => format!(
                        "    {}  {}",
                        report::pad(&dep.name, width),
                        paint(version, DIM, color)
                    ),
                    None => format!("    {}", dep.name),
                };
                lines.push(row.trim_end().to_string());
            }
        }
    }

    lines
}

const DIM: u8 = 8;
/// Yellow, the same accent every group heading gets whichever toolchain it is
/// under — the toolchain is already coloured, and a second cycling palette
/// inside it would say nothing.
const CONTEXT: u8 = 3;

#[cfg(test)]
mod tests {
    use super::super::detect::PackageManager;
    use super::*;

    fn dep(name: &str, version: Option<&str>) -> Dependency {
        Dependency::new(name, version.map(str::to_string))
    }

    fn context(groups: &[Group], name: &str) -> Vec<Dependency> {
        groups
            .iter()
            .find(|group| group.context == name)
            .map(|group| group.deps.clone())
            .unwrap_or_else(|| panic!("no `{name}` group in {:?}", contexts(groups)))
    }

    fn contexts(groups: &[Group]) -> Vec<&str> {
        groups.iter().map(|group| group.context.as_str()).collect()
    }

    #[test]
    fn mise_reads_a_version_a_list_of_them_and_a_table() {
        let groups = mise_toml(
            r#"
[tools]
node = "22"
python = ["3.12", "3.13"]
rust = { version = "1.85", profile = "minimal" }
"#,
        );

        assert_eq!(
            context(&groups, "tools"),
            vec![
                dep("node", Some("22")),
                dep("python", Some("3.12 3.13")),
                dep("rust", Some("1.85")),
            ]
        );
    }

    #[test]
    fn tool_versions_reads_the_asdf_format() {
        let groups = tool_versions("# pinned\nnodejs 22.1.0\nruby 3.4.1 3.3.6\n\n");

        assert_eq!(
            context(&groups, "tools"),
            vec![
                dep("nodejs", Some("22.1.0")),
                dep("ruby", Some("3.4.1 3.3.6"))
            ]
        );
    }

    #[test]
    fn cargo_separates_the_three_kinds_of_dependency() {
        let groups = cargo(
            r#"
[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
local = { path = "../local" }
forked = { git = "https://example.test/x" }
shared = { workspace = true }

[dev-dependencies]
tempfile = "3"

[build-dependencies]
cc = "1.0"
"#,
        );

        assert_eq!(
            contexts(&groups),
            vec!["dependencies", "dev-dependencies", "build-dependencies"]
        );
        assert_eq!(
            context(&groups, "dependencies"),
            vec![
                dep("anyhow", Some("1")),
                dep("clap", Some("4")),
                dep("forked", Some("https://example.test/x")),
                dep("local", Some("../local")),
                dep("shared", Some("workspace")),
            ]
        );
        assert_eq!(
            context(&groups, "dev-dependencies"),
            vec![dep("tempfile", Some("3"))]
        );
    }

    #[test]
    fn a_cargo_workspace_lists_the_versions_its_members_inherit() {
        let groups = cargo("[workspace.dependencies]\nserde = \"1\"\n");
        assert_eq!(
            context(&groups, "workspace.dependencies"),
            vec![dep("serde", Some("1"))]
        );
    }

    #[test]
    fn go_tells_what_it_requires_from_what_it_pulled_in() {
        let groups = go_mod(
            r#"
module example.test/x

go 1.24

require github.com/spf13/cobra v1.8.1

require (
	github.com/stretchr/testify v1.10.0
	golang.org/x/sys v0.28.0 // indirect
)
"#,
        );

        assert_eq!(
            context(&groups, "require"),
            vec![
                dep("github.com/spf13/cobra", Some("v1.8.1")),
                dep("github.com/stretchr/testify", Some("v1.10.0")),
            ]
        );
        assert_eq!(
            context(&groups, "indirect"),
            vec![dep("golang.org/x/sys", Some("v0.28.0"))]
        );
    }

    #[test]
    fn a_go_module_with_no_requirements_declares_nothing() {
        assert_eq!(go_mod("module example.test/x\n\ngo 1.24\n"), vec![]);
    }

    #[test]
    fn package_json_keeps_each_kind_of_dependency_apart() {
        let groups = package_json(
            r#"{
              "dependencies": { "react": "^19.0.0" },
              "devDependencies": { "typescript": "^5.7.0", "vitest": "^2.1.0" },
              "peerDependencies": { "react-dom": "^19.0.0" }
            }"#,
        );

        assert_eq!(
            contexts(&groups),
            vec!["dependencies", "devDependencies", "peerDependencies"]
        );
        assert_eq!(
            context(&groups, "devDependencies"),
            vec![
                dep("typescript", Some("^5.7.0")),
                dep("vitest", Some("^2.1.0"))
            ]
        );
    }

    #[test]
    fn deno_lists_its_import_map_through_the_comments() {
        let groups = deno_json(
            "{\n  // the standard library\n  \"imports\": { \"@std/assert\": \"jsr:@std/assert@^1.0.0\" },\n}",
        );

        assert_eq!(
            context(&groups, "imports"),
            vec![dep("@std/assert", Some("jsr:@std/assert@^1.0.0"))]
        );
    }

    #[test]
    fn clojure_reads_the_deps_and_each_alias_that_adds_some() {
        let groups = deps_edn(
            r#"{:paths ["src"]
                :deps {org.clojure/clojure {:mvn/version "1.12.0"}
                       io.github.x/y {:git/tag "v1.2.0" :git/sha "abc1234"}
                       local/lib {:local/root "../lib"}}
                :aliases {:test {:extra-deps {lambdaisland/kaocha {:mvn/version "1.91.1392"}}}
                          :build {:extra-paths ["build"]}}}"#,
        );

        assert_eq!(contexts(&groups), vec!["deps", "aliases :test"]);
        assert_eq!(
            context(&groups, "deps"),
            vec![
                dep("io.github.x/y", Some("v1.2.0")),
                dep("local/lib", Some("../lib")),
                dep("org.clojure/clojure", Some("1.12.0")),
            ]
        );
        assert_eq!(
            context(&groups, "aliases :test"),
            vec![dep("lambdaisland/kaocha", Some("1.91.1392"))]
        );
    }

    #[test]
    fn maven_groups_by_scope_and_keeps_managed_versions_apart() {
        let groups = pom(r#"<project>
              <!-- <dependency><artifactId>commented</artifactId></dependency> -->
              <properties><junit.version>5.11.4</junit.version></properties>
              <dependencyManagement>
                <dependencies>
                  <dependency>
                    <groupId>com.fasterxml.jackson</groupId>
                    <artifactId>jackson-bom</artifactId>
                    <version>2.18.2</version>
                  </dependency>
                </dependencies>
              </dependencyManagement>
              <dependencies>
                <dependency>
                  <groupId>org.slf4j</groupId>
                  <artifactId>slf4j-api</artifactId>
                  <version>2.0.16</version>
                </dependency>
                <dependency>
                  <groupId>org.junit.jupiter</groupId>
                  <artifactId>junit-jupiter</artifactId>
                  <version>${junit.version}</version>
                  <scope>test</scope>
                </dependency>
              </dependencies>
            </project>"#);

        assert_eq!(
            context(&groups, "managed"),
            vec![dep("com.fasterxml.jackson:jackson-bom", Some("2.18.2"))]
        );
        assert_eq!(
            context(&groups, "compile"),
            vec![dep("org.slf4j:slf4j-api", Some("2.0.16"))]
        );
        assert_eq!(
            context(&groups, "test"),
            vec![dep("org.junit.jupiter:junit-jupiter", Some("5.11.4"))],
            "the property was not resolved"
        );
    }

    #[test]
    fn gradle_groups_by_the_configuration_each_was_declared_under() {
        let groups = gradle(
            r#"
plugins { id("java") }

repositories { mavenCentral() }

dependencies {
    implementation("org.slf4j:slf4j-api:2.0.16")
    implementation(platform("com.fasterxml.jackson:jackson-bom:2.18.2"))
    testImplementation 'org.junit.jupiter:junit-jupiter:5.11.4'
    implementation(libs.guava)
}

tasks.test { useJUnitPlatform() }
"#,
        );

        assert_eq!(
            contexts(&groups),
            vec!["implementation", "testImplementation"]
        );
        assert_eq!(
            context(&groups, "implementation"),
            vec![
                dep("com.fasterxml.jackson:jackson-bom", Some("2.18.2")),
                dep("org.slf4j:slf4j-api", Some("2.0.16")),
            ]
        );
        assert_eq!(
            context(&groups, "testImplementation"),
            vec![dep("org.junit.jupiter:junit-jupiter", Some("5.11.4"))]
        );
    }

    #[test]
    fn a_coordinate_outside_the_dependencies_block_is_not_a_dependency() {
        assert_eq!(gradle("classpath(\"a:b:1\")\ndependencies {\n}\n"), vec![]);
    }

    #[test]
    fn pyproject_reads_every_list_a_project_can_keep() {
        let groups = pyproject(
            r#"
[build-system]
requires = ["hatchling"]

[project]
name = "x"
dependencies = ["httpx[http2]>=0.27,<1", "rich"]

[project.optional-dependencies]
cli = ["typer >=0.15"]

[dependency-groups]
dev = ["pytest>=8"]
"#,
        );

        assert_eq!(
            contexts(&groups),
            vec![
                "dependencies",
                "optional-dependencies.cli",
                "dependency-groups.dev",
                "build-system"
            ]
        );
        assert_eq!(
            context(&groups, "dependencies"),
            vec![dep("httpx[http2]", Some(">=0.27,<1")), dep("rich", None)]
        );
        assert_eq!(
            context(&groups, "optional-dependencies.cli"),
            vec![dep("typer", Some(">=0.15"))]
        );
        assert_eq!(
            context(&groups, "dependency-groups.dev"),
            vec![dep("pytest", Some(">=8"))]
        );
        assert_eq!(
            context(&groups, "build-system"),
            vec![dep("hatchling", None)]
        );
    }

    #[test]
    fn a_poetry_project_keeps_its_dependencies_where_poetry_puts_them() {
        let groups =
            pyproject("[tool.poetry.dependencies]\npython = \"^3.13\"\nrequests = \"^2.32\"\n");

        assert_eq!(
            context(&groups, "tool.poetry.dependencies"),
            vec![dep("python", Some("^3.13")), dep("requests", Some("^2.32"))]
        );
    }

    #[test]
    fn requirements_skips_the_lines_that_are_options_rather_than_packages() {
        let groups =
            requirements("-r base.txt\n-e .\n\n# a comment\nrequests==2.32.3  # pinned\nrich\n");

        assert_eq!(
            context(&groups, "requirements"),
            vec![dep("requests", Some("==2.32.3")), dep("rich", None)]
        );
    }

    #[test]
    fn terraform_lists_the_providers_it_pins_and_the_version_it_asks_for() {
        let groups = terraform(
            r#"
terraform {
  required_version = ">= 1.9"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    random = {
      source = "hashicorp/random"
    }
  }
}

resource "aws_s3_bucket" "b" {}
"#,
        );

        assert_eq!(
            context(&groups, "required_version"),
            vec![dep("terraform", Some(">= 1.9"))]
        );
        assert_eq!(
            context(&groups, "required_providers"),
            vec![
                dep("hashicorp/aws", Some("~> 5.0")),
                dep("hashicorp/random", None),
            ]
        );
    }

    #[test]
    fn a_manifest_that_will_not_parse_costs_the_listing_that_manifest_alone() {
        assert_eq!(cargo("[dependencies"), vec![]);
        assert_eq!(package_json("{ not json"), vec![]);
        assert_eq!(pyproject("!!"), vec![]);
        assert_eq!(deps_edn(""), vec![]);
    }

    #[test]
    fn a_detected_toolchain_is_listed_even_where_its_manifest_was_unreadable() {
        let scan = Scan::default();
        let detections = [Detection {
            toolchain: Toolchain::Node(PackageManager::Bun),
            evidence: "package.json".to_string(),
        }];

        assert_eq!(
            list(&detections, &scan),
            vec![Manifest {
                toolchain: "bun",
                source: "package.json".to_string(),
                groups: vec![],
            }]
        );
    }

    #[test]
    fn each_toolchain_is_read_from_the_file_it_keeps_its_dependencies_in() {
        let mut scan = Scan::default();
        scan.contents.insert(
            "Cargo.toml".to_string(),
            "[dependencies]\nanyhow = \"1\"".to_string(),
        );
        scan.contents.insert("main.tf".to_string(), String::new());
        scan.paths.insert("main.tf".to_string());

        let detections = [
            Detection {
                toolchain: Toolchain::Rust,
                evidence: "Cargo.toml".to_string(),
            },
            Detection {
                toolchain: Toolchain::Terraform,
                evidence: "main.tf".to_string(),
            },
        ];
        let manifests = list(&detections, &scan);

        assert_eq!(manifests[0].source, "Cargo.toml");
        assert_eq!(manifests[0].groups.len(), 1);
        assert_eq!(manifests[1].source, "*.tf", "several modules read as one");
    }

    #[test]
    fn the_listing_indents_a_group_under_its_manifest_and_a_dependency_under_that() {
        let manifests = [Manifest {
            toolchain: "rust",
            source: "Cargo.toml".to_string(),
            groups: vec![Group::new(
                "dependencies",
                vec![dep("anyhow", Some("1")), dep("local", None)],
            )],
        }];

        assert_eq!(
            listing(&manifests, false),
            vec![
                "rust  Cargo.toml",
                "  dependencies",
                "    anyhow  1",
                "    local",
            ]
        );
    }

    #[test]
    fn a_manifest_that_declares_nothing_still_says_it_was_looked_at() {
        let manifests = [Manifest {
            toolchain: "gradle",
            source: "build.gradle.kts".to_string(),
            groups: vec![],
        }];

        assert_eq!(
            listing(&manifests, false),
            vec!["gradle  build.gradle.kts", "  nothing declared"]
        );
    }

    #[test]
    fn manifests_are_separated_by_a_blank_line() {
        let manifests = [
            Manifest {
                toolchain: "rust",
                source: "Cargo.toml".to_string(),
                groups: vec![],
            },
            Manifest {
                toolchain: "go",
                source: "go.mod".to_string(),
                groups: vec![],
            },
        ];

        assert_eq!(listing(&manifests, false)[2], "");
    }

    #[test]
    fn a_requirement_is_split_into_the_package_and_what_it_asks_of_it() {
        assert_eq!(requirement("rich"), dep("rich", None));
        assert_eq!(requirement("httpx >= 0.27"), dep("httpx", Some(">= 0.27")));
        assert_eq!(
            requirement("uv[test]==0.5.11 ; python_version < '3.13'"),
            dep("uv[test]", Some("==0.5.11 ; python_version < '3.13'"))
        );
        assert_eq!(
            requirement("x @ git+https://example.test/x"),
            dep("x", Some("@ git+https://example.test/x"))
        );
    }
}
