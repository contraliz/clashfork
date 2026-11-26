use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_EXPORT_BYTES: usize = 1_000_000;
const MAX_FILE_NAME_CHARS: usize = 120;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveExportResult {
    pub canceled: bool,
    pub file_name: Option<String>,
}

pub fn normalize_suggested_name(input: &str) -> String {
    let trimmed = input.trim();
    let stem = if trimmed.to_ascii_lowercase().ends_with(".json") {
        &trimmed[..trimmed.len() - 5]
    } else {
        trimmed
    };
    let mut name: String = stem
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '-'
            } else {
                character
            }
        })
        .collect();
    name = name
        .trim_matches(|character: char| character == ' ' || character == '.')
        .to_owned();
    if name.is_empty()
        || name
            .chars()
            .all(|character| matches!(character, '-' | ' ' | '.'))
    {
        name = "clashfork-routing".to_owned();
    }
    if is_windows_reserved_name(&name) {
        name = format!("clashfork-{name}");
    }
    let suffix = ".json";
    let keep = MAX_FILE_NAME_CHARS.saturating_sub(suffix.len());
    if name.chars().count() > keep {
        name = name
            .chars()
            .take(keep)
            .collect::<String>()
            .trim_end_matches(|character: char| character == ' ' || character == '.')
            .to_owned();
    }
    if name.is_empty() {
        name = "clashfork-routing".to_owned();
    }
    format!("{name}{suffix}")
}

fn is_windows_reserved_name(name: &str) -> bool {
    let upper = name.trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

pub fn validate_routing_export(contents: &str) -> Result<Value> {
    if contents.len() > MAX_EXPORT_BYTES {
        bail!("Routing export files must be under 1 MB.");
    }
    let document: Value =
        serde_json::from_str(contents).context("The routing export is not valid JSON.")?;
    let object = document
        .as_object()
        .context("The routing export must be a JSON object.")?;
    if object.get("format").and_then(Value::as_str) != Some("clashfork-routing") {
        bail!("The routing export has an unsupported format.");
    }
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        bail!("The routing export has an unsupported version.");
    }
    match object.get("type").and_then(Value::as_str) {
        Some("rule") => {
            if !object.get("rule").is_some_and(Value::is_object) {
                bail!("The routing export is missing its rule.");
            }
        }
        Some("ruleset") => {
            if !object.get("rules").is_some_and(Value::is_array) {
                bail!("The routing export is missing its rules.");
            }
        }
        _ => bail!("The routing export has an unsupported type."),
    }
    Ok(document)
}

pub fn write_export_file(path: &Path, contents: &str) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("json") {
        bail!("Choose a .json filename for the routing export.");
    }
    std::fs::write(path, contents.as_bytes())
        .with_context(|| format!("Writing {} failed.", path.display()))?;
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn choose_export_path(owner: Option<isize>, suggested_name: &str) -> Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut file_buffer = vec![0u16; 32_768];
    for (slot, unit) in file_buffer
        .iter_mut()
        .zip(std::ffi::OsStr::new(suggested_name).encode_wide())
    {
        *slot = unit;
    }
    let filter: Vec<u16> = "Clashfork routing JSON\0*.json\0All files\0*.*\0\0"
        .encode_utf16()
        .collect();
    let default_extension: Vec<u16> = "json\0".encode_utf16().collect();
    let title: Vec<u16> = "Save Clashfork routing file\0".encode_utf16().collect();

    let mut dialog = OPENFILENAMEW::default();
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner.map_or(std::ptr::null_mut(), |value| value as HWND);
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file_buffer.as_mut_ptr();
    dialog.nMaxFile = file_buffer.len() as u32;
    dialog.lpstrDefExt = default_extension.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;

    let selected = unsafe { GetSaveFileNameW(&mut dialog) };
    if selected == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            bail!("The Save As dialog failed (code {error}).")
        };
    }
    let length = file_buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(file_buffer.len());
    let path = PathBuf::from(String::from_utf16_lossy(&file_buffer[..length]));
    if path.as_os_str().is_empty() {
        bail!("The Save As dialog returned no filename.");
    }
    Ok(Some(path))
}

#[cfg(not(windows))]
fn choose_export_path(_owner: Option<isize>, _suggested_name: &str) -> Result<Option<PathBuf>> {
    bail!("Native Save As is available only in the Windows desktop app.")
}

fn finish_export(path: Option<PathBuf>, contents: &str) -> Result<SaveExportResult> {
    let Some(path) = path else {
        return Ok(SaveExportResult {
            canceled: true,
            file_name: None,
        });
    };
    write_export_file(&path, contents)?;
    Ok(SaveExportResult {
        canceled: false,
        file_name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    })
}

pub fn save_routing_export(
    owner: Option<isize>,
    suggested_name: &str,
    contents: &str,
) -> Result<SaveExportResult> {
    validate_routing_export(contents)?;
    let suggested_name = normalize_suggested_name(suggested_name);
    finish_export(choose_export_path(owner, &suggested_name)?, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULESET: &str = r#"{
  "format": "clashfork-routing",
  "version": 1,
  "type": "ruleset",
  "name": "QA ruleset",
  "rules": [
    {
      "name": "QA rule",
      "conditions": [{ "kind": "domain_suffix", "value": "qa.example" }],
      "destination": { "kind": "proxy", "proxyRef": "proxy-1", "label": "Proxy 1" }
    }
  ],
  "miscellaneous": { "kind": "proxy", "proxyRef": "proxy-2", "label": "Proxy 2" }
}"#;

    #[test]
    fn writes_exact_saved_bytes_and_accepts_both_document_types() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ruleset.json");
        validate_routing_export(RULESET).unwrap();
        write_export_file(&path, RULESET).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), RULESET);

        let rule = r#"{"format":"clashfork-routing","version":1,"type":"rule","rule":{"name":"One","conditions":[{"kind":"domain","value":"qa.example"}],"destination":{"kind":"direct"}}}"#;
        validate_routing_export(rule).unwrap();
    }

    #[test]
    fn rejects_invalid_documents_and_non_json_destinations() {
        assert!(validate_routing_export("not json").is_err());
        assert!(
            validate_routing_export(r#"{"format":"other","version":1,"type":"rule","rule":{}}"#)
                .is_err()
        );
        assert!(
            validate_routing_export(
                r#"{"format":"clashfork-routing","version":2,"type":"rule","rule":{}}"#
            )
            .is_err()
        );
        assert!(
            validate_routing_export(r#"{"format":"clashfork-routing","version":1,"type":"rule"}"#)
                .is_err()
        );

        let directory = tempfile::tempdir().unwrap();
        assert!(write_export_file(&directory.path().join("ruleset.txt"), RULESET).is_err());
    }

    #[test]
    fn cancellation_writes_nothing_and_write_errors_propagate() {
        let canceled = finish_export(None, RULESET).unwrap();
        assert!(canceled.canceled);
        assert_eq!(canceled.file_name, None);

        let directory = tempfile::tempdir().unwrap();
        let written = finish_export(Some(directory.path().join("ruleset.json")), RULESET).unwrap();
        assert!(!written.canceled);
        assert_eq!(written.file_name.as_deref(), Some("ruleset.json"));

        let failed = finish_export(Some(directory.path().to_path_buf()), RULESET);
        assert!(
            failed.is_err(),
            "a non-json destination must surface an error"
        );
    }

    #[test]
    fn normalizes_suggested_file_names() {
        assert_eq!(normalize_suggested_name("QA: ruleset"), "QA- ruleset.json");
        assert_eq!(
            normalize_suggested_name("  ...  "),
            "clashfork-routing.json"
        );
        assert_eq!(normalize_suggested_name("***"), "clashfork-routing.json");
        assert_eq!(normalize_suggested_name("Already.JSON"), "Already.json");
        assert_eq!(normalize_suggested_name("trailing. . "), "trailing.json");
        assert_eq!(normalize_suggested_name("CON"), "clashfork-CON.json");
        assert_eq!(normalize_suggested_name("lpt1.json"), "clashfork-lpt1.json");
        assert_eq!(
            normalize_suggested_name("a/b\\c:d*e?f\"g<h>i|j"),
            "a-b-c-d-e-f-g-h-i-j.json"
        );
        let long = normalize_suggested_name(&"a".repeat(200));
        assert!(long.chars().count() <= MAX_FILE_NAME_CHARS);
        assert!(long.ends_with(".json"));
    }
}
