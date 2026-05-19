use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Debug)]
pub struct TestSelection {
    pub name: String,
    pub initial_state_path: PathBuf,
    pub prompt_path: PathBuf,
    pub initial_state_sha256: String,
    pub prompt_sha256: String,
}

pub fn load_test_selection(name: &str) -> Result<TestSelection, String> {
    let test_dir = PathBuf::from("tests").join(name);
    if !test_dir.is_dir() {
        return Err(format!(
            "test '{name}' does not exist at {}",
            test_dir.display()
        ));
    }

    let initial_state = test_dir.join("initial_state.zip");
    if !initial_state.is_file() {
        return Err(format!(
            "test '{name}' is missing required file {}",
            initial_state.display()
        ));
    }

    let prompt = test_dir.join("PROMPT.md");
    if !prompt.is_file() {
        return Err(format!(
            "test '{name}' is missing required file {}",
            prompt.display()
        ));
    }

    Ok(TestSelection {
        name: name.to_owned(),
        initial_state_path: initial_state.clone(),
        prompt_path: prompt.clone(),
        initial_state_sha256: sha256_file(&initial_state)?,
        prompt_sha256: sha256_file(&prompt)?,
    })
}

// ── Temp prompt ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct TempPrompt {
    pub path: PathBuf,
}

pub fn prepare_temp_prompt(run_id: &str, prompt_path: &Path) -> Result<TempPrompt, String> {
    let temp_dir = std::env::temp_dir().join("harness-test-prompts");
    fs::create_dir_all(&temp_dir).map_err(|error| {
        format!(
            "failed to create temporary prompt directory {}: {error}",
            temp_dir.display()
        )
    })?;

    let path = temp_dir.join(format!("{run_id}-PROMPT.md"));
    fs::copy(prompt_path, &path).map_err(|error| {
        format!(
            "failed to copy prompt {} to temporary prompt {}: {error}",
            prompt_path.display(),
            path.display()
        )
    })?;

    Ok(TempPrompt { path })
}

pub fn copy_prompt_artifact(prompt: &TempPrompt, run_dir: &Path) -> Result<String, String> {
    let artifact_path = run_dir.join("PROMPT.md");
    fs::copy(&prompt.path, &artifact_path).map_err(|error| {
        format!(
            "failed to copy temporary prompt {} to artifact {}: {error}",
            prompt.path.display(),
            artifact_path.display()
        )
    })?;

    Ok("PROMPT.md".to_owned())
}

pub fn remove_temp_prompt(prompt: &TempPrompt) -> Result<(), String> {
    fs::remove_file(&prompt.path).map_err(|error| {
        format!(
            "failed to remove temporary prompt {}: {error}",
            prompt.path.display()
        )
    })
}

// ── Archive extraction ──────────────────────────────────────────────────────

pub fn extract_initial_state(zip_path: &Path, working_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(working_dir).map_err(|error| {
        format!(
            "failed to create working directory {}: {error}",
            working_dir.display()
        )
    })?;

    let file = File::open(zip_path)
        .map_err(|error| format!("failed to open {}: {error}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("failed to read zip {}: {error}", zip_path.display()))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to read zip entry {index}: {error}"))?;
        let enclosed_name = entry
            .enclosed_name()
            .ok_or_else(|| format!("unsafe zip entry path: {}", entry.name()))?
            .to_owned();

        validate_archive_path(&enclosed_name)?;
        reject_symlink_entry(&entry)?;

        let output_path = working_dir.join(&enclosed_name);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| {
                format!(
                    "failed to create extracted directory {}: {error}",
                    output_path.display()
                )
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create extracted parent directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        let mut output = File::create(&output_path).map_err(|error| {
            format!(
                "failed to create extracted file {}: {error}",
                output_path.display()
            )
        })?;
        io::copy(&mut entry, &mut output).map_err(|error| {
            format!(
                "failed to extract {} to {}: {error}",
                entry.name(),
                output_path.display()
            )
        })?;
    }

    Ok(())
}

pub fn validate_archive_path(path: &Path) -> Result<(), String> {
    if path.is_absolute() {
        return Err(format!(
            "unsafe absolute zip entry path: {}",
            path.display()
        ));
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "unsafe zip entry path escapes working_dir: {}",
            path.display()
        ));
    }

    Ok(())
}

fn reject_symlink_entry(entry: &zip::read::ZipFile<'_>) -> Result<(), String> {
    const UNIX_FILE_TYPE_MASK: u32 = 0o170000;
    const UNIX_SYMLINK: u32 = 0o120000;

    if entry
        .unix_mode()
        .is_some_and(|mode| mode & UNIX_FILE_TYPE_MASK == UNIX_SYMLINK)
    {
        return Err(format!("unsafe symlink zip entry: {}", entry.name()));
    }

    Ok(())
}

// ── Hashing ─────────────────────────────────────────────────────────────────

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to open {} for hashing: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to read {} for hashing: {error}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}
