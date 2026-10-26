use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, InlineTable, Item, Value};

pub const MANIFEST_FILE: &str = "orvi.toml";
pub const LOCK_FILE: &str = "orvi.lock";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectManifest {
    pub name: String,
    pub version: String,
    pub entry: PathBuf,
    pub dependencies: BTreeMap<String, PathDependency>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PathDependency {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Lockfile {
    pub lock: LockMetadata,
    #[serde(rename = "package")]
    pub packages: Vec<LockedPackage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LockMetadata {
    pub version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    pub entry: String,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: ProjectManifest,
}

#[derive(Debug)]
pub struct ProjectError {
    pub kind: ProjectErrorKind,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectErrorKind {
    ManifestNotFound,
    ManifestIo,
    ManifestSyntax,
    InvalidName,
    InvalidVersion,
    InvalidEntry,
    EntryMissing,
    SourceIo,
    InvalidDependency,
    DependencyMissing,
    DependencyNameMismatch,
    DependencyCycle,
    LockIo,
    LockSyntax,
}

#[derive(Deserialize)]
struct ManifestDocument {
    project: ManifestProject,
    #[serde(default)]
    dependencies: BTreeMap<String, PathDependency>,
}

#[derive(Deserialize)]
struct ManifestProject {
    name: String,
    version: String,
    entry: PathBuf,
}

impl Project {
    pub fn discover(start: &Path) -> Result<Self, ProjectError> {
        let start = if start.is_file() {
            start.parent().unwrap_or(start)
        } else {
            start
        };
        let root = start
            .ancestors()
            .find(|directory| directory.join(MANIFEST_FILE).is_file())
            .ok_or_else(|| {
                ProjectError::new(
                    ProjectErrorKind::ManifestNotFound,
                    format!(
                        "could not find '{MANIFEST_FILE}' from '{}' or its parents",
                        start.display()
                    ),
                )
            })?;
        Self::load(root)
    }

    pub fn load(root: &Path) -> Result<Self, ProjectError> {
        let manifest_path = root.join(MANIFEST_FILE);
        let source = fs::read_to_string(&manifest_path).map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::ManifestIo,
                format!(
                    "could not read project manifest '{}': {error}",
                    manifest_path.display()
                ),
            )
        })?;
        let document = toml::from_str::<ManifestDocument>(&source).map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::ManifestSyntax,
                format!(
                    "invalid project manifest '{}': {error}",
                    manifest_path.display()
                ),
            )
        })?;
        if document.project.name.trim().is_empty() {
            return Err(ProjectError::new(
                ProjectErrorKind::InvalidName,
                "project name cannot be empty",
            ));
        }
        if document.project.version.trim().is_empty() {
            return Err(ProjectError::new(
                ProjectErrorKind::InvalidVersion,
                "project version cannot be empty",
            ));
        }
        validate_entry(&document.project.entry)?;
        for (name, dependency) in &document.dependencies {
            validate_package_name(name)?;
            validate_dependency_path(&dependency.path)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            manifest_path,
            manifest: ProjectManifest {
                name: document.project.name,
                version: document.project.version,
                entry: document.project.entry,
                dependencies: document.dependencies,
            },
        })
    }

    pub fn entry_path(&self) -> PathBuf {
        self.root.join(&self.manifest.entry)
    }

    pub fn existing_entry_path(&self) -> Result<PathBuf, ProjectError> {
        let entry = self.entry_path();
        if !entry.is_file() {
            return Err(ProjectError::new(
                ProjectErrorKind::EntryMissing,
                format!(
                    "project entry '{}' does not exist or is not a file",
                    entry.display()
                ),
            ));
        }
        Ok(entry)
    }

    pub fn source_files(&self) -> Result<Vec<PathBuf>, ProjectError> {
        let conventional_source = self.root.join("src");
        let source_root = if conventional_source.is_dir() {
            conventional_source
        } else {
            self.entry_path()
                .parent()
                .unwrap_or(&self.root)
                .to_path_buf()
        };
        let mut files = Vec::new();
        collect_source_files(&source_root, &mut files)?;
        files.sort();
        files.dedup();
        Ok(files)
    }

    pub fn dependency(&self, name: &str) -> Result<Project, ProjectError> {
        validate_package_name(name)?;
        let dependency = self.manifest.dependencies.get(name).ok_or_else(|| {
            ProjectError::new(
                ProjectErrorKind::DependencyMissing,
                format!(
                    "project '{}' has no dependency named '{name}'",
                    self.manifest.name
                ),
            )
        })?;
        validate_dependency_path(&dependency.path)?;
        let dependency_root = self
            .root
            .join(&dependency.path)
            .canonicalize()
            .map_err(|error| {
                ProjectError::new(
                    ProjectErrorKind::DependencyMissing,
                    format!(
                        "could not resolve dependency '{name}' at '{}': {error}",
                        self.root.join(&dependency.path).display()
                    ),
                )
            })?;
        let dependency_project = Project::load(&dependency_root)?;
        if dependency_project.manifest.name != name {
            return Err(ProjectError::new(
                ProjectErrorKind::DependencyNameMismatch,
                format!(
                    "dependency key '{name}' does not match package name '{}'",
                    dependency_project.manifest.name
                ),
            ));
        }
        Ok(dependency_project)
    }

    pub fn resolve_dependencies(&self) -> Result<Lockfile, ProjectError> {
        let mut packages = BTreeMap::new();
        let mut paths = HashMap::new();
        let mut stack = Vec::new();
        resolve_project_dependencies(self, &mut packages, &mut paths, &mut stack)?;
        Ok(Lockfile {
            lock: LockMetadata { version: 1 },
            packages: packages.into_values().collect(),
        })
    }

    pub fn write_lockfile(&self) -> Result<Lockfile, ProjectError> {
        let lockfile = self.resolve_dependencies()?;
        let encoded = toml::to_string_pretty(&lockfile).map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::LockSyntax,
                format!("could not encode lockfile: {error}"),
            )
        })?;
        let path = self.root.join(LOCK_FILE);
        fs::write(&path, encoded).map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::LockIo,
                format!("could not write lockfile '{}': {error}", path.display()),
            )
        })?;
        Ok(lockfile)
    }

    pub fn read_lockfile(&self) -> Result<Lockfile, ProjectError> {
        let path = self.root.join(LOCK_FILE);
        let source = fs::read_to_string(&path).map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::LockIo,
                format!("could not read lockfile '{}': {error}", path.display()),
            )
        })?;
        let lockfile = toml::from_str::<Lockfile>(&source).map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::LockSyntax,
                format!("invalid lockfile '{}': {error}", path.display()),
            )
        })?;
        if lockfile.lock.version != 1 {
            return Err(ProjectError::new(
                ProjectErrorKind::LockSyntax,
                format!("unsupported lockfile version {}", lockfile.lock.version),
            ));
        }
        Ok(lockfile)
    }
}

impl ProjectError {
    fn new(kind: ProjectErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self.kind {
            ProjectErrorKind::ManifestNotFound => "ORVI-PROJECT-001",
            ProjectErrorKind::ManifestIo => "ORVI-PROJECT-002",
            ProjectErrorKind::ManifestSyntax => "ORVI-PROJECT-003",
            ProjectErrorKind::InvalidName => "ORVI-PROJECT-004",
            ProjectErrorKind::InvalidVersion => "ORVI-PROJECT-005",
            ProjectErrorKind::InvalidEntry => "ORVI-PROJECT-006",
            ProjectErrorKind::EntryMissing => "ORVI-PROJECT-007",
            ProjectErrorKind::SourceIo => "ORVI-PROJECT-008",
            ProjectErrorKind::InvalidDependency => "ORVI-PROJECT-009",
            ProjectErrorKind::DependencyMissing => "ORVI-PROJECT-010",
            ProjectErrorKind::DependencyNameMismatch => "ORVI-PROJECT-011",
            ProjectErrorKind::DependencyCycle => "ORVI-PROJECT-012",
            ProjectErrorKind::LockIo => "ORVI-PROJECT-013",
            ProjectErrorKind::LockSyntax => "ORVI-PROJECT-014",
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl Error for ProjectError {}

pub fn add_path_dependency(
    project: &Project,
    name: &str,
    dependency_path: &Path,
) -> Result<Project, ProjectError> {
    validate_package_name(name)?;
    let absolute = if dependency_path.is_absolute() {
        dependency_path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(dependency_path)
    };
    let dependency_root = absolute.canonicalize().map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::DependencyMissing,
            format!(
                "could not resolve dependency '{name}' at '{}': {error}",
                absolute.display()
            ),
        )
    })?;
    let dependency = Project::load(&dependency_root)?;
    if dependency.manifest.name != name {
        return Err(ProjectError::new(
            ProjectErrorKind::DependencyNameMismatch,
            format!(
                "dependency key '{name}' does not match package name '{}'",
                dependency.manifest.name
            ),
        ));
    }
    let root = project
        .root
        .canonicalize()
        .unwrap_or_else(|_| project.root.clone());
    if root == dependency_root {
        return Err(ProjectError::new(
            ProjectErrorKind::DependencyCycle,
            "a project cannot depend on itself",
        ));
    }
    let relative = pathdiff::diff_paths(&dependency_root, &root).unwrap_or(dependency_root);
    let mut prospective = project.clone();
    prospective.manifest.dependencies.insert(
        name.to_owned(),
        PathDependency {
            path: relative.clone(),
        },
    );
    prospective.resolve_dependencies()?;
    update_manifest_dependency(&project.manifest_path, name, Some(&relative))?;
    Project::load(&project.root)
}

pub fn remove_dependency(project: &Project, name: &str) -> Result<Project, ProjectError> {
    validate_package_name(name)?;
    if !project.manifest.dependencies.contains_key(name) {
        return Err(ProjectError::new(
            ProjectErrorKind::DependencyMissing,
            format!(
                "project '{}' has no dependency named '{name}'",
                project.manifest.name
            ),
        ));
    }
    update_manifest_dependency(&project.manifest_path, name, None)?;
    Project::load(&project.root)
}

pub fn resolve_import(module_path: &Path, source: &str) -> Result<PathBuf, ProjectError> {
    if source.starts_with("std:") {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            "standard modules do not resolve to files",
        ));
    }
    if let Some(package_source) = source.strip_prefix("pkg:") {
        let (name, requested) = package_source
            .split_once('/')
            .map_or((package_source, None), |(name, path)| (name, Some(path)));
        validate_package_name(name)?;
        let project = Project::discover(module_path.parent().unwrap_or(module_path))?;
        let dependency = project.dependency(name)?;
        let candidate = match requested {
            Some(path) if !path.is_empty() => {
                let relative = Path::new(path);
                validate_package_module_path(relative)?;
                dependency.root.join(relative)
            }
            _ => dependency.entry_path(),
        };
        return contained_source_file(&dependency.root, &candidate, source);
    }
    if source.contains(':') {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            format!("unsupported module source '{source}'"),
        ));
    }
    let candidate = module_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(source);
    if candidate
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("orvi")
    {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            "local module path must end with '.orvi'",
        ));
    }
    candidate.canonicalize().map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::DependencyMissing,
            format!(
                "could not resolve module '{}': {error}",
                candidate.display()
            ),
        )
    })
}

fn update_manifest_dependency(
    manifest_path: &Path,
    name: &str,
    path: Option<&Path>,
) -> Result<(), ProjectError> {
    let source = fs::read_to_string(manifest_path).map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::ManifestIo,
            format!(
                "could not read project manifest '{}': {error}",
                manifest_path.display()
            ),
        )
    })?;
    let mut document = source.parse::<DocumentMut>().map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::ManifestSyntax,
            format!(
                "invalid project manifest '{}': {error}",
                manifest_path.display()
            ),
        )
    })?;
    if let Some(path) = path {
        if !document.contains_key("dependencies") {
            document["dependencies"] = Item::Table(toml_edit::Table::new());
        }
        let mut dependency = InlineTable::new();
        dependency.insert("path", Value::from(path_to_manifest(path)));
        document["dependencies"][name] = Item::Value(Value::InlineTable(dependency));
    } else if let Some(dependencies) = document
        .get_mut("dependencies")
        .and_then(Item::as_table_mut)
    {
        dependencies.remove(name);
    }
    fs::write(manifest_path, document.to_string()).map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::ManifestIo,
            format!(
                "could not update project manifest '{}': {error}",
                manifest_path.display()
            ),
        )
    })
}

fn resolve_project_dependencies(
    project: &Project,
    packages: &mut BTreeMap<String, LockedPackage>,
    paths: &mut HashMap<String, PathBuf>,
    stack: &mut Vec<PathBuf>,
) -> Result<(), ProjectError> {
    let project_root = project
        .root
        .canonicalize()
        .unwrap_or_else(|_| project.root.clone());
    if let Some(position) = stack.iter().position(|path| path == &project_root) {
        let mut cycle = stack[position..]
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(project_root.display().to_string());
        return Err(ProjectError::new(
            ProjectErrorKind::DependencyCycle,
            format!("dependency cycle: {}", cycle.join(" -> ")),
        ));
    }
    stack.push(project_root);
    for (name, spec) in &project.manifest.dependencies {
        let dependency = project.dependency(name)?;
        let dependency_root = dependency
            .root
            .canonicalize()
            .unwrap_or_else(|_| dependency.root.clone());
        if let Some(existing) = paths.get(name) {
            if existing != &dependency_root {
                return Err(ProjectError::new(
                    ProjectErrorKind::DependencyNameMismatch,
                    format!(
                        "package '{name}' resolves to both '{}' and '{}'",
                        existing.display(),
                        dependency_root.display()
                    ),
                ));
            }
        } else {
            paths.insert(name.clone(), dependency_root);
        }
        packages
            .entry(name.clone())
            .or_insert_with(|| LockedPackage {
                name: name.clone(),
                version: dependency.manifest.version.clone(),
                source: format!(
                    "path+{}:{}",
                    project.manifest.name,
                    path_to_manifest(&spec.path)
                ),
                entry: path_to_manifest(&dependency.manifest.entry),
                dependencies: dependency.manifest.dependencies.keys().cloned().collect(),
            });
        resolve_project_dependencies(&dependency, packages, paths, stack)?;
    }
    stack.pop();
    Ok(())
}

fn validate_package_name(name: &str) -> Result<(), ProjectError> {
    let valid = !name.is_empty()
        && name.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric() || character == '_' || (character == '-' && index > 0)
        })
        && !name.starts_with(|character: char| character.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            format!("invalid package name '{name}'"),
        ))
    }
}

fn validate_dependency_path(path: &Path) -> Result<(), ProjectError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            "dependency path must be relative and cannot be empty",
        ))
    } else {
        Ok(())
    }
}

fn validate_package_module_path(path: &Path) -> Result<(), ProjectError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        || path.extension().and_then(|extension| extension.to_str()) != Some("orvi")
    {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            "package module must be a relative '.orvi' path contained in the package",
        ));
    }
    Ok(())
}

fn contained_source_file(
    root: &Path,
    candidate: &Path,
    source: &str,
) -> Result<PathBuf, ProjectError> {
    let root = root.canonicalize().map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::DependencyMissing,
            format!(
                "could not resolve package root '{}': {error}",
                root.display()
            ),
        )
    })?;
    let candidate = candidate.canonicalize().map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::DependencyMissing,
            format!("could not resolve module '{source}': {error}"),
        )
    })?;
    if !candidate.starts_with(&root)
        || candidate
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("orvi")
        || !candidate.is_file()
    {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidDependency,
            format!("module '{source}' is not an '.orvi' file contained in its package"),
        ));
    }
    Ok(candidate)
}

fn path_to_manifest(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn validate_entry(entry: &Path) -> Result<(), ProjectError> {
    if entry.as_os_str().is_empty() {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidEntry,
            "project entry cannot be empty",
        ));
    }
    if entry.is_absolute()
        || entry
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidEntry,
            "project entry must be a relative path contained in the project",
        ));
    }
    if entry.extension().and_then(|extension| extension.to_str()) != Some("orvi") {
        return Err(ProjectError::new(
            ProjectErrorKind::InvalidEntry,
            "project entry must end with '.orvi'",
        ));
    }
    Ok(())
}

fn collect_source_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), ProjectError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        ProjectError::new(
            ProjectErrorKind::SourceIo,
            format!(
                "could not read source directory '{}': {error}",
                directory.display()
            ),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::SourceIo,
                format!(
                    "could not read an entry in '{}': {error}",
                    directory.display()
                ),
            )
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            ProjectError::new(
                ProjectErrorKind::SourceIo,
                format!("could not inspect '{}': {error}", path.display()),
            )
        })?;
        if file_type.is_dir() {
            let hidden = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'));
            if !hidden {
                collect_source_files(&path, files)?;
            }
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("orvi")
        {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Project, ProjectErrorKind, add_path_dependency, remove_dependency, resolve_import,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn project(manifest: &str, with_entry: bool) -> std::path::PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("orvi-project-{id}"));
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("orvi.toml"), manifest).unwrap();
        if with_entry {
            fs::write(root.join("src/main.orvi"), "print(42)").unwrap();
        }
        root
    }

    #[test]
    fn loads_and_discovers_a_project() {
        let root = project(
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
            true,
        );
        let loaded = Project::discover(&root.join("src/nested")).unwrap();
        assert_eq!(loaded.manifest.name, "demo");
        assert_eq!(
            loaded.existing_entry_path().unwrap(),
            root.join("src/main.orvi")
        );
        fs::write(
            root.join("src/nested/module.orvi"),
            "export const value = 1",
        )
        .unwrap();
        assert_eq!(loaded.source_files().unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_entries_outside_the_project() {
        let root = project(
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nentry = \"../main.orvi\"\n",
            false,
        );
        let error = Project::load(&root).unwrap_err();
        assert_eq!(error.kind, ProjectErrorKind::InvalidEntry);
        assert_eq!(error.code(), "ORVI-PROJECT-006");
        assert!(error.to_string().contains("contained in the project"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_missing_and_invalid_manifests() {
        let root = project("[project]\nname = \"demo\"", false);
        let invalid = Project::load(&root).unwrap_err();
        assert_eq!(invalid.kind, ProjectErrorKind::ManifestSyntax);
        assert!(invalid.to_string().contains("ORVI-PROJECT-003"));
        fs::remove_file(root.join("orvi.toml")).unwrap();
        let missing = Project::discover(&root).unwrap_err();
        assert_eq!(missing.kind, ProjectErrorKind::ManifestNotFound);
        assert!(missing.to_string().contains("ORVI-PROJECT-001"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adds_locks_resolves_and_removes_path_dependencies() {
        let root = project(
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
            true,
        );
        let dependency_root = root.parent().unwrap().join(format!(
            "orvi-package-math-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dependency_root.join("src")).unwrap();
        fs::write(
            dependency_root.join("orvi.toml"),
            "[project]\nname = \"math\"\nversion = \"1.2.3\"\nentry = \"src/main.orvi\"\n",
        )
        .unwrap();
        fs::write(
            dependency_root.join("src/main.orvi"),
            "export fn double(value: int) -> int { return value * 2 }",
        )
        .unwrap();
        fs::write(
            dependency_root.join("src/extra.orvi"),
            "export const answer = 42",
        )
        .unwrap();

        let project = Project::load(&root).unwrap();
        let project = add_path_dependency(&project, "math", &dependency_root).unwrap();
        assert!(project.manifest.dependencies.contains_key("math"));
        let lockfile = project.write_lockfile().unwrap();
        assert_eq!(lockfile.packages.len(), 1);
        assert_eq!(lockfile.packages[0].name, "math");
        assert_eq!(project.read_lockfile().unwrap(), lockfile);
        assert_eq!(
            resolve_import(&root.join("src/main.orvi"), "pkg:math").unwrap(),
            dependency_root
                .join("src/main.orvi")
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            resolve_import(&root.join("src/main.orvi"), "pkg:math/src/extra.orvi").unwrap(),
            dependency_root
                .join("src/extra.orvi")
                .canonicalize()
                .unwrap()
        );
        assert!(
            resolve_import(&root.join("src/main.orvi"), "pkg:math/../app/src/main.orvi").is_err()
        );
        let project = remove_dependency(&project, "math").unwrap();
        assert!(project.manifest.dependencies.is_empty());

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(dependency_root).unwrap();
    }

    #[test]
    fn rejects_dependency_cycles() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("orvi-cycle-{id}"));
        let app = base.join("app");
        let library = base.join("library");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::create_dir_all(library.join("src")).unwrap();
        fs::write(
            app.join("orvi.toml"),
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n\n[dependencies]\nlibrary = { path = \"../library\" }\n",
        )
        .unwrap();
        fs::write(
            library.join("orvi.toml"),
            "[project]\nname = \"library\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n\n[dependencies]\napp = { path = \"../app\" }\n",
        )
        .unwrap();
        fs::write(app.join("src/main.orvi"), "print(1)").unwrap();
        fs::write(library.join("src/main.orvi"), "print(2)").unwrap();
        let error = Project::load(&app)
            .unwrap()
            .resolve_dependencies()
            .unwrap_err();
        assert_eq!(error.kind, ProjectErrorKind::DependencyCycle);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn rejects_invalid_dependency_declarations() {
        let root = project(
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n\n[dependencies]\n\"bad/name\" = { path = \"../dep\" }\n",
            true,
        );
        let error = Project::load(&root).unwrap_err();
        assert_eq!(error.kind, ProjectErrorKind::InvalidDependency);
        assert_eq!(error.code(), "ORVI-PROJECT-009");
        fs::remove_dir_all(root).unwrap();
    }
}
