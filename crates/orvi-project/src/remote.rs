use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{MANIFEST_FILE, Project, ProjectError, ProjectErrorKind, contained_source_file};

const MAX_PACKAGE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoteDependency {
    pub url: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    manifest: String,
    files: BTreeMap<String, String>,
}

impl RemoteDependency {
    pub(super) fn validate(&self) -> Result<(), ProjectError> {
        let valid_url = Url::parse(&self.url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        });
        if !valid_url {
            return Err(remote_error(
                "remote packages require an HTTPS URL without credentials, query, or fragment",
            ));
        }
        if !valid_digest(&self.sha256) {
            return Err(remote_error(
                "remote packages require a lowercase 64-digit SHA-256 digest",
            ));
        }
        Ok(())
    }

    pub(super) fn lock_source(&self) -> String {
        format!("{}#sha256={}", self.url, self.sha256)
    }

    pub(super) fn package_root(&self, name: &str, download: bool) -> Result<PathBuf, ProjectError> {
        let cache =
            cache_dir().ok_or_else(|| remote_error("could not locate the package cache"))?;
        let root = cache.join(&self.sha256);
        if root.join(MANIFEST_FILE).is_file() {
            return Ok(root);
        }
        if !download {
            return Err(remote_error(format!(
                "package '{name}' is missing from the cache; run 'orviscript install'"
            )));
        }

        let mut client = reqwest::blocking::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30));
        for certificate in rustls_native_certs::load_native_certs().certs {
            if let Ok(certificate) = reqwest::Certificate::from_der(&certificate) {
                client = client.add_root_certificate(certificate);
            }
        }
        let client = client
            .build()
            .map_err(|_| remote_error("could not initialize the HTTPS package client"))?;
        let response = client.get(&self.url).send().map_err(|error| {
            remote_error(format!(
                "could not download package '{name}': {}",
                error.without_url()
            ))
        })?;
        if !response.status().is_success() {
            return Err(remote_error(format!(
                "download for package '{name}' returned HTTP {}",
                response.status()
            )));
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_PACKAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| remote_error(format!("could not read download for package '{name}'")))?;
        if bytes.len() as u64 > MAX_PACKAGE_BYTES {
            return Err(remote_error(format!(
                "package '{name}' exceeds the download size limit"
            )));
        }
        self.unpack(name, &bytes, &cache, &root)?;
        Ok(root)
    }

    fn unpack(
        &self,
        name: &str,
        bytes: &[u8],
        cache: &Path,
        root: &Path,
    ) -> Result<(), ProjectError> {
        if format!("{:x}", Sha256::digest(bytes)) != self.sha256 {
            return Err(remote_error(format!(
                "SHA-256 mismatch for package '{name}'"
            )));
        }
        let bundle: Bundle = serde_json::from_slice(bytes)
            .map_err(|_| remote_error(format!("invalid JSON bundle for package '{name}'")))?;
        fs::create_dir_all(cache).map_err(cache_error)?;
        let staged = tempfile::Builder::new()
            .prefix("staging-")
            .tempdir_in(cache)
            .map_err(cache_error)?;
        fs::write(staged.path().join(MANIFEST_FILE), bundle.manifest).map_err(cache_error)?;
        for (path, contents) in bundle.files {
            if !valid_source_path(&path) {
                return Err(remote_error(format!(
                    "invalid source path in package '{name}'"
                )));
            }
            let file = staged.path().join(path);
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).map_err(cache_error)?;
            }
            let mut output = fs::File::create_new(file).map_err(cache_error)?;
            std::io::Write::write_all(&mut output, contents.as_bytes()).map_err(cache_error)?;
        }
        let project = Project::load(staged.path())?;
        if project.manifest.name != name
            || project.manifest.exports.is_none()
            || project
                .manifest
                .dependencies
                .values()
                .any(|dep| matches!(dep, super::Dependency::Path(_)))
        {
            return Err(remote_error(format!(
                "package '{name}' must match its declared name, declare [exports], and use only remote dependencies"
            )));
        }
        contained_source_file(staged.path(), &project.entry_path(), name)?;
        if let Some(exports) = &project.manifest.exports {
            for (alias, target) in exports {
                contained_source_file(staged.path(), &staged.path().join(target), alias)?;
            }
        }
        match fs::rename(staged.path(), root) {
            Ok(()) => Ok(()),
            Err(_) if root.join(MANIFEST_FILE).is_file() => Ok(()),
            Err(error) => Err(cache_error(error)),
        }
    }
}

fn cache_dir() -> Option<PathBuf> {
    env::var_os("ORVI_PACKAGE_CACHE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            let base = env::var_os(if cfg!(windows) {
                "LOCALAPPDATA"
            } else {
                "XDG_CACHE_HOME"
            })
            .map(PathBuf::from)
            .or_else(|| env::home_dir().map(|home| home.join(".cache")))?;
            Some(base.join("orviscript").join("packages"))
        })
}

pub(super) fn containing_package(module_path: &Path) -> Option<PathBuf> {
    let cache = cache_dir()?.canonicalize().ok()?;
    let module = module_path.canonicalize().ok()?;
    let first = module.strip_prefix(&cache).ok()?.components().next()?;
    valid_digest(first.as_os_str().to_str()?).then(|| cache.join(first))
}

pub(super) fn is_cached_package(root: &Path) -> bool {
    containing_package(root).is_some()
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_source_path(path: &str) -> bool {
    path.ends_with(".orvi")
        && path.split('/').all(|part| {
            let stem = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            !stem.is_empty()
                && !part.ends_with('.')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
        })
}

fn cache_error(error: std::io::Error) -> ProjectError {
    let message = if error.kind() == ErrorKind::AlreadyExists {
        "package cache contains conflicting paths".to_owned()
    } else {
        format!("could not write package cache: {error}")
    };
    remote_error(message)
}

fn remote_error(message: impl Into<String>) -> ProjectError {
    ProjectError::new(ProjectErrorKind::RemotePackage, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_and_stages_remote_packages() {
        let cache = tempfile::tempdir().unwrap();
        let manifest =
            "[project]\nname = 'library'\nversion = '1.0.0'\nentry = 'src/main.orvi'\n[exports]\n";
        for path in ["../escape.orvi", "src/COM1.orvi", "src/main.orvi"] {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "manifest": manifest,
                "files": { (path): "export const answer = 42" },
            }))
            .unwrap();
            let mut spec = RemoteDependency {
                url: "https://example.invalid/library.json".into(),
                version: "^1.0".into(),
                sha256: "0".repeat(64),
            };
            let root = cache.path().join(format!("{:x}", Sha256::digest(&bytes)));
            let error = spec
                .unpack("library", &bytes, cache.path(), &root)
                .unwrap_err();
            assert!(error.to_string().contains("SHA-256 mismatch"));
            assert!(!root.exists());
            spec.sha256 = format!("{:x}", Sha256::digest(&bytes));

            let result = spec.unpack("library", &bytes, cache.path(), &root);
            if path == "src/main.orvi" {
                result.unwrap();
                assert!(Project::load(&root).unwrap().existing_entry_path().is_ok());
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("invalid source path")
                );
                assert!(!root.exists());
            }
        }
    }
}
