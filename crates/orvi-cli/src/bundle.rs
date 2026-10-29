use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const SLOT_MAGIC: &[u8] = b"ORVI_BUNDLE_SLOT_V001";
const FORMAT_VERSION: u32 = 1;
const LANGUAGE_VERSION_BYTES: usize = 16;
const CHECKSUM_BYTES: usize = 32;
const SLOT_BYTES: usize = 1024 * 1024;
const HEADER_BYTES: usize = SLOT_MAGIC.len() + 4 + 8 + CHECKSUM_BYTES + LANGUAGE_VERSION_BYTES;
const PAYLOAD_CAPACITY: usize = SLOT_BYTES - HEADER_BYTES - SLOT_MAGIC.len();

const fn empty_slot() -> [u8; SLOT_BYTES] {
    let mut slot = [0_u8; SLOT_BYTES];
    let mut index = 0;
    while index < SLOT_MAGIC.len() {
        slot[index] = SLOT_MAGIC[index];
        slot[SLOT_BYTES - SLOT_MAGIC.len() + index] = SLOT_MAGIC[index];
        index += 1;
    }
    slot
}

// The builder replaces this fixed-size region in a copied executable. Keeping
// the payload inside the image avoids the very slow PE overlay startup path on
// Windows while retaining a single-file artifact.
#[used]
static BUNDLE_SLOT: [u8; SLOT_BYTES] = empty_slot();

#[derive(Debug, Eq, PartialEq)]
pub struct Bundle {
    pub format_version: u32,
    pub language_version: String,
    pub source: String,
    pub checksum: [u8; CHECKSUM_BYTES],
}

pub fn read_current() -> Result<Option<Bundle>, String> {
    parse_slot(&BUNDLE_SLOT)
}

pub fn read(path: &Path) -> Result<Option<Bundle>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| format!("could not read bundle '{}': {error}", path.display()))?;
    let Some(offset) = find_slot(&bytes) else {
        return Ok(None);
    };
    parse_slot(&bytes[offset..offset + SLOT_BYTES])
}

fn parse_slot(slot: &[u8]) -> Result<Option<Bundle>, String> {
    let mut cursor = SLOT_MAGIC.len();
    let format_version = read_u32(slot, &mut cursor);
    if format_version == 0 {
        return Ok(None);
    }
    if format_version != FORMAT_VERSION {
        return Err(format!(
            "unsupported bundle format {format_version}, expected {FORMAT_VERSION}"
        ));
    }
    let source_length = read_u64(slot, &mut cursor);
    let source_length = usize::try_from(source_length)
        .map_err(|_| "bundle source is too large for this platform".to_owned())?;
    if source_length > PAYLOAD_CAPACITY {
        return Err("bundle source length exceeds the reserved payload capacity".into());
    }
    let mut checksum = [0_u8; CHECKSUM_BYTES];
    checksum.copy_from_slice(&slot[cursor..cursor + CHECKSUM_BYTES]);
    cursor += CHECKSUM_BYTES;
    let version_bytes = &slot[cursor..cursor + LANGUAGE_VERSION_BYTES];
    cursor += LANGUAGE_VERSION_BYTES;
    let version_end = version_bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(version_bytes.len());
    let language_version = std::str::from_utf8(&version_bytes[..version_end])
        .map_err(|_| "bundle contains an invalid language version".to_owned())?
        .to_owned();
    let source = &slot[cursor..cursor + source_length];
    let actual = Sha256::digest(source);
    if actual.as_slice() != checksum {
        return Err("bundle checksum does not match its embedded source".into());
    }
    let source =
        String::from_utf8(source.to_vec()).map_err(|_| "bundle source is not valid UTF-8")?;
    Ok(Some(Bundle {
        format_version,
        language_version,
        source,
        checksum,
    }))
}

pub fn write(
    runtime: &Path,
    output: &Path,
    source: &str,
    language_version: &str,
    force: bool,
) -> Result<(), String> {
    if output.exists() && !force {
        return Err(format!(
            "output '{}' already exists; pass --force to replace it",
            output.display()
        ));
    }
    if source.len() > PAYLOAD_CAPACITY {
        return Err(format!(
            "source is {} bytes, exceeding the standalone limit of {PAYLOAD_CAPACITY} bytes",
            source.len()
        ));
    }
    if language_version.len() > LANGUAGE_VERSION_BYTES {
        return Err("language version is too long for the bundle format".into());
    }
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create output directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    let temporary = temporary_path(output)?;
    let result = write_temporary(runtime, &temporary, source, language_version)
        .and_then(|()| replace_output(&temporary, output, force));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_temporary(
    runtime: &Path,
    temporary: &Path,
    source: &str,
    language_version: &str,
) -> Result<(), String> {
    fs::copy(runtime, temporary).map_err(|error| {
        format!(
            "could not copy runtime '{}' to '{}': {error}",
            runtime.display(),
            temporary.display()
        )
    })?;
    let bytes = fs::read(temporary)
        .map_err(|error| format!("could not inspect temporary bundle: {error}"))?;
    let offset = find_slot(&bytes).ok_or_else(|| {
        "runtime does not contain the reserved OrviScript bundle slot; rebuild the CLI".to_owned()
    })?;
    let mut slot = BUNDLE_SLOT.to_vec();
    let mut cursor = SLOT_MAGIC.len();
    slot[cursor..cursor + 4].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    cursor += 4;
    slot[cursor..cursor + 8].copy_from_slice(&(source.len() as u64).to_le_bytes());
    cursor += 8;
    let checksum = Sha256::digest(source.as_bytes());
    slot[cursor..cursor + CHECKSUM_BYTES].copy_from_slice(&checksum);
    cursor += CHECKSUM_BYTES;
    slot[cursor..cursor + language_version.len()].copy_from_slice(language_version.as_bytes());
    cursor += LANGUAGE_VERSION_BYTES;
    slot[cursor..cursor + source.len()].copy_from_slice(source.as_bytes());

    let mut file = OpenOptions::new()
        .write(true)
        .open(temporary)
        .map_err(|error| format!("could not open temporary bundle: {error}"))?;
    file.seek(SeekFrom::Start(offset as u64))
        .and_then(|_| file.write_all(&slot))
        .map_err(|error| format!("could not write bundle slot: {error}"))?;
    drop(file);
    update_windows_pe_checksum(temporary)?;
    OpenOptions::new()
        .write(true)
        .open(temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("could not finish bundle: {error}"))
}

#[cfg(windows)]
fn update_windows_pe_checksum(path: &Path) -> Result<(), String> {
    let mut bytes = fs::read(path).map_err(|error| format!("could not read PE image: {error}"))?;
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return Ok(());
    }
    let pe_offset = usize::try_from(u32::from_le_bytes(
        bytes[0x3c..0x40].try_into().expect("four-byte PE offset"),
    ))
    .map_err(|_| "PE header offset does not fit this platform")?;
    let checksum_offset = pe_offset
        .checked_add(24 + 64)
        .filter(|offset| offset + 4 <= bytes.len())
        .ok_or_else(|| "Windows PE checksum field is outside the executable".to_owned())?;
    if bytes.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        return Err("standalone runtime has an invalid Windows PE signature".into());
    }
    bytes[checksum_offset..checksum_offset + 4].fill(0);
    let mut checksum = 0_u64;
    for chunk in bytes.chunks(2) {
        let word = u16::from_le_bytes([chunk[0], *chunk.get(1).unwrap_or(&0)]);
        checksum += u64::from(word);
        checksum = (checksum & 0xffff) + (checksum >> 16);
    }
    checksum = (checksum & 0xffff) + (checksum >> 16);
    checksum += checksum >> 16;
    let checksum = ((checksum & 0xffff) + bytes.len() as u64) as u32;
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| format!("could not open PE image for checksum update: {error}"))?;
    file.seek(SeekFrom::Start(checksum_offset as u64))
        .and_then(|_| file.write_all(&checksum.to_le_bytes()))
        .map_err(|error| format!("could not update Windows PE checksum: {error}"))
}

#[cfg(not(windows))]
fn update_windows_pe_checksum(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn find_slot(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < SLOT_BYTES {
        return None;
    }
    bytes
        .windows(SLOT_MAGIC.len())
        .enumerate()
        .find_map(|(offset, candidate)| {
            let end = offset.checked_add(SLOT_BYTES)?;
            (candidate == SLOT_MAGIC
                && end <= bytes.len()
                && &bytes[end - SLOT_MAGIC.len()..end] == SLOT_MAGIC)
                .then_some(offset)
        })
}

fn replace_output(temporary: &Path, output: &Path, force: bool) -> Result<(), String> {
    if !output.exists() {
        return fs::rename(temporary, output).map_err(|error| {
            format!(
                "could not move completed bundle to '{}': {error}",
                output.display()
            )
        });
    }
    if !force {
        return Err(format!(
            "output '{}' already exists; pass --force to replace it",
            output.display()
        ));
    }
    let backup = temporary_path(output)?.with_extension("orvi-backup");
    fs::rename(output, &backup).map_err(|error| {
        format!(
            "could not preserve existing output '{}': {error}",
            output.display()
        )
    })?;
    if let Err(error) = fs::rename(temporary, output) {
        let _ = fs::rename(&backup, output);
        return Err(format!(
            "could not replace output '{}': {error}",
            output.display()
        ));
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

pub fn checksum_hex(checksum: &[u8; CHECKSUM_BYTES]) -> String {
    checksum.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn temporary_path(output: &Path) -> Result<PathBuf, String> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("orviscript-bundle");
    for attempt in 0..100 {
        let candidate = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => {
                fs::remove_file(&candidate)
                    .map_err(|error| format!("could not prepare temporary bundle: {error}"))?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not reserve temporary bundle: {error}")),
        }
    }
    Err("could not reserve a temporary bundle path".into())
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> u32 {
    let value = u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().unwrap());
    *offset += 4;
    value
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> u64 {
    let value = u64::from_le_bytes(bytes[*offset..*offset + 8].try_into().unwrap());
    *offset += 8;
    value
}

#[cfg(test)]
mod tests {
    use super::{BUNDLE_SLOT, HEADER_BYTES, SLOT_BYTES, SLOT_MAGIC, read, write};
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn paths() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("orvi-bundle-format-{id}"));
        fs::create_dir_all(&directory).unwrap();
        let runtime = directory.join("runtime");
        fs::write(&runtime, BUNDLE_SLOT.as_slice()).unwrap();
        (runtime, directory.join("output"), directory)
    }

    #[test]
    fn round_trips_and_validates_the_payload_checksum() {
        let (runtime, output, directory) = paths();
        write(&runtime, &output, "print(42);", "0.10.1", false).unwrap();
        let bundle = read(&output).unwrap().unwrap();
        assert_eq!(bundle.source, "print(42);");
        assert_eq!(bundle.language_version, "0.10.1");

        let mut file = OpenOptions::new().write(true).open(&output).unwrap();
        file.seek(SeekFrom::Start(HEADER_BYTES as u64)).unwrap();
        file.write_all(b"X").unwrap();
        assert!(read(&output).unwrap_err().contains("checksum"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let (runtime, output, directory) = paths();
        fs::write(&output, b"existing").unwrap();
        assert!(write(&runtime, &output, "print(1);", "0.10.1", false).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"existing");
        write(&runtime, &output, "print(2);", "0.10.1", true).unwrap();
        assert_eq!(read(&output).unwrap().unwrap().source, "print(2);");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_unknown_versions_and_treats_truncated_slots_as_non_bundles() {
        let (runtime, output, directory) = paths();
        write(&runtime, &output, "print(42);", "0.10.1", false).unwrap();
        let mut file = OpenOptions::new().write(true).open(&output).unwrap();
        file.seek(SeekFrom::Start(SLOT_MAGIC.len() as u64)).unwrap();
        file.write_all(&2_u32.to_le_bytes()).unwrap();
        assert!(
            read(&output)
                .unwrap_err()
                .contains("unsupported bundle format")
        );

        file.set_len((SLOT_BYTES - 1) as u64).unwrap();
        drop(file);
        assert!(read(&output).unwrap().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn produces_identical_bundles_for_identical_inputs() {
        let (runtime, output, directory) = paths();
        let second = directory.join("output-second");
        write(&runtime, &output, "print(42);", "0.10.1", false).unwrap();
        write(&runtime, &second, "print(42);", "0.10.1", false).unwrap();
        assert_eq!(fs::read(&output).unwrap(), fs::read(&second).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn preserves_executable_permissions_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let (runtime, output, directory) = paths();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        write(&runtime, &output, "print(42);", "0.10.1", false).unwrap();
        assert_ne!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o111,
            0
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
