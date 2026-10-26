use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

pub const MANIFEST_FILE: &str = "orvi.toml";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectManifest {
    pub name: String,
    pub version: String,
    pub entry: PathBuf,
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
}

#[derive(Deserialize)]
struct ManifestDocument {
    project: ManifestProject,
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
        Ok(Self {
            root: root.to_path_buf(),
            manifest_path,
            manifest: ProjectManifest {
                name: document.project.name,
                version: document.project.version,
                entry: document.project.entry,
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
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl Error for ProjectError {}

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
    use super::{Project, ProjectErrorKind};
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
}
