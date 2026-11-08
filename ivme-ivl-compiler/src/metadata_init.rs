use crate::{
    error::CompileError,
    metadata::{decode_ascii_decimal, validate_extended_metadata, LevelMetadata},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

pub(crate) fn initialize_file(path: &Path, is_vmf: bool) -> Result<bool, CompileError> {
    let original =
        fs::read(path).map_err(|e| CompileError::MapUnreadable(path.to_path_buf(), e))?;
    let source = std::str::from_utf8(&original)
        .map_err(|e| CompileError::InvalidExtendedMetadata(format!("invalid UTF-8: {e}")))?;
    let uuid = uuid::Uuid::new_v4().to_string();
    let output = initialize_source(source, is_vmf, &uuid)?;
    if output.as_bytes() == original {
        return Ok(false);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = parent.join(format!("{filename}.{}.tmp", uuid));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| CompileError::OutputWriteFailed(temp.clone(), e))?;
    let result = file
        .write_all(output.as_bytes())
        .and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = result {
        let _ = fs::remove_file(&temp);
        return Err(CompileError::OutputWriteFailed(temp, e));
    }
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(CompileError::OutputWriteFailed(path.to_path_buf(), e));
    }
    Ok(true)
}

#[derive(Clone)]
struct Token {
    value: String,
    start: usize,
    end: usize,
    quoted: bool,
}
#[derive(Clone)]
struct Prop {
    key: String,
    value: String,
    value_start: usize,
    value_end: usize,
    key_start: usize,
}
fn invalid() -> CompileError {
    CompileError::InvalidExtendedMetadata("ambiguous or malformed initialization source".into())
}
fn lex(s: &str) -> Result<Vec<Token>, CompileError> {
    let b = s.as_bytes();
    let mut i = if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        3
    } else {
        0
    };
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'{' || b[i] == b'}' {
            out.push(Token {
                value: (b[i] as char).to_string(),
                start: i,
                end: i + 1,
                quoted: false,
            });
            i += 1;
            continue;
        }
        if b[i] == b'"' {
            let start = i;
            i += 1;
            let content = i;
            let mut value = Vec::new();
            let mut closed = false;
            while i < b.len() {
                if b[i] == b'\\' {
                    if i + 1 >= b.len() {
                        return Err(invalid());
                    }
                    value.push(b[i]);
                    i += 1;
                    value.push(b[i]);
                    i += 1;
                } else if b[i] == b'"' {
                    closed = true;
                    break;
                } else {
                    value.push(b[i]);
                    i += 1;
                }
            }
            if !closed {
                return Err(invalid());
            }
            let end = i;
            i += 1;
            out.push(Token {
                value: String::from_utf8(value).map_err(|_| invalid())?,
                start: content,
                end,
                quoted: true,
            });
            let _ = start;
            continue;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'{' && b[i] != b'}' {
            i += 1;
        }
        out.push(Token {
            value: s[start..i].into(),
            start,
            end: i,
            quoted: false,
        });
    }
    Ok(out)
}

pub(crate) fn initialize_source(
    source: &str,
    is_vmf: bool,
    replacement_uuid: &str,
) -> Result<String, CompileError> {
    let t = lex(source)?;
    let mut depth = 0usize;
    let mut roots = Vec::new();
    let mut spans = Vec::new();
    for (i, x) in t.iter().enumerate() {
        if !x.quoted && x.value == "{" {
            if depth == 0 {
                spans.push((i, depth));
            }
            depth += 1;
        } else if !x.quoted && x.value == "}" {
            if depth == 0 {
                return Err(invalid());
            }
            depth -= 1;
            if depth == 0 {
                let (a, _) = spans.pop().ok_or_else(invalid)?;
                roots.push((a, i));
            }
        }
    }
    if depth != 0 {
        return Err(invalid());
    }
    let mut found = Vec::new();
    for &(a, b) in &roots {
        if is_vmf && (a == 0 || t[a - 1].quoted || t[a - 1].value != "entity") {
            continue;
        }
        let mut props = Vec::new();
        let mut d = 1usize;
        let mut j = a + 1;
        while j < b {
            let x = &t[j];
            if !x.quoted && x.value == "{" {
                d += 1;
                j += 1;
                continue;
            }
            if !x.quoted && x.value == "}" {
                d -= 1;
                j += 1;
                continue;
            }
            if d == 1 && x.quoted && j + 1 < b && t[j + 1].quoted {
                let v = &t[j + 1];
                props.push(Prop {
                    key: x.value.clone(),
                    value: v.value.clone(),
                    value_start: v.start,
                    value_end: v.end,
                    key_start: x.start,
                });
                j += 2;
            } else {
                j += 1;
            }
        }
        let classes: Vec<_> = props.iter().filter(|p| p.key == "classname").collect();
        if classes.iter().any(|p| p.value == "info_level_metadata") {
            if classes.len() != 1 {
                return Err(invalid());
            }
            if is_vmf && a == 0 { /* VMF entity recognized by its preceding unquoted entity token below */
            }
            found.push((a, b, props));
        }
    }
    if is_vmf {
        found.retain(|(a, _, _)| *a > 0 && t[*a - 1].value == "entity" && !t[*a - 1].quoted);
    }
    if found.is_empty() {
        return Err(CompileError::NoMetadataEntity);
    }
    if found.len() != 1 {
        return Err(CompileError::DuplicateMetadata);
    }
    let (a, b, props) = found.remove(0);
    let names = [
        "metadata_schema_version",
        "level_uuid",
        "bronze_time_ms",
        "silver_time_ms",
        "gold_time_ms",
    ];
    for k in names {
        if props.iter().filter(|p| p.key == k).count() > 1 {
            return Err(invalid());
        }
    }
    let get = |k: &str| props.iter().find(|p| p.key == k);
    let schema = get(names[0]);
    let uuid = get(names[1]);
    let medals = [get(names[2]), get(names[3]), get(names[4])];
    if let Some(p) = schema {
        let n = decode_ascii_decimal(&p.value).ok_or_else(invalid)?;
        if n > 1 {
            return Err(invalid());
        }
    }
    let empty_uuid = uuid.map_or(true, |p| {
        p.value
            .trim_matches(|c: char| c.is_ascii() && (c as u32) <= 32)
            .is_empty()
    });
    let mut meta = LevelMetadata {
        course_id: String::new(),
        level_name: String::new(),
        level_creator: String::new(),
        level_description: String::new(),
        game_mode: String::new(),
        checkpoints: 0,
        metadata_schema_version: Some("1".into()),
        level_uuid: Some(if empty_uuid {
            replacement_uuid.into()
        } else {
            uuid.unwrap().value.clone()
        }),
        bronze_time_ms: None,
        silver_time_ms: None,
        gold_time_ms: None,
    };
    if let Some(p) = schema {
        if decode_ascii_decimal(&p.value) == Some(1) {
            meta.metadata_schema_version = Some(p.value.clone())
        }
    }
    if medals.iter().all(|p| p.is_none()) {
        meta.bronze_time_ms = Some("0".into());
        meta.silver_time_ms = Some("0".into());
        meta.gold_time_ms = Some("0".into());
    } else if medals.iter().any(|p| p.is_none()) {
        return Err(invalid());
    } else {
        meta.bronze_time_ms = Some(medals[0].unwrap().value.clone());
        meta.silver_time_ms = Some(medals[1].unwrap().value.clone());
        meta.gold_time_ms = Some(medals[2].unwrap().value.clone());
    }
    validate_extended_metadata(&meta)?;
    let mut edits = Vec::<(usize, usize, String)>::new();
    for (p, val) in [
        (schema, meta.metadata_schema_version.as_deref().unwrap()),
        (
            if empty_uuid { uuid } else { None },
            meta.level_uuid.as_deref().unwrap(),
        ),
    ] {
        if let Some(p) = p {
            if p.value != val {
                edits.push((p.value_start, p.value_end, val.into()));
            }
        }
    }
    let nl = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let first = props.first().map(|p| p.key_start).unwrap_or(t[a].start);
    let ls = source[..first].rfind('\n').map_or(0, |n| n + 1);
    let indent = source[ls..first]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>();
    let close = t[b].start;
    let line = source[..close].rfind('\n').map_or(close, |n| n + 1);
    let mut insert = String::new();
    for (idx, k) in names.iter().enumerate() {
        if get(k).is_none() {
            let val = if idx == 0 {
                "1"
            } else if idx == 1 {
                replacement_uuid
            } else {
                "0"
            };
            insert.push_str(&format!("{}\"{}\" \"{}\"{}", indent, k, val, nl));
        }
    }
    if !insert.is_empty() {
        edits.push((line, line, insert));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = source.to_owned();
    for (s, e, v) in edits {
        out.replace_range(s..e, &v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::initialize_source;

    const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const UUID2: &str = "550e8400-e29b-41d4-a716-446655440001";

    fn fixture(vmf: bool, properties: &str) -> String {
        format!(
            "{}{{\n\"classname\" \"info_level_metadata\"\n{} }}\n",
            if vmf { "entity\n" } else { "" },
            properties
        )
    }

    fn initialized(vmf: bool, props: &str, uuid: &str) -> String {
        let prefix = if vmf { "entity\n" } else { "" };
        format!("{}{{\n\"classname\" \"info_level_metadata\"\n{}\"metadata_schema_version\" \"1\"\n\"level_uuid\" \"{}\"\n\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n }}\n", prefix, props, uuid)
    }

    #[test]
    fn test_initialize_map_and_vmf_once() {
        for vmf in [false, true] {
            let source = fixture(vmf, "");
            let expected = initialized(vmf, "", UUID);
            let actual = initialize_source(&source, vmf, UUID).unwrap();
            assert_eq!(actual, expected);
            let second = initialize_source(&actual, vmf, UUID2).unwrap();
            assert_eq!(second, expected);
        }
    }

    #[test]
    fn test_initialize_preserves_source_bytes() {
        for vmf in [false, true] {
            let prefix = if vmf { "entity\r\n" } else { "" };
            let original = format!("\u{feff}{}{{\r\n\"classname\" \"info_level_metadata\"\r\n// {{ nested }}\r\n\"description\" \"caf\u{e9} {{ \\\"quoted\\\" }}\"\r\n\"other\" \"x\\\"y\"\r\n{{\r\n\"level_uuid\" \"nested\"\r\n}}\r\n}}\r\n", prefix);
            let snapshot = original.clone();
            let out = initialize_source(&original, vmf, UUID).unwrap();
            let lines = [
                format!("\"metadata_schema_version\" \"1\"\r\n"),
                format!("\"level_uuid\" \"{}\"\r\n", UUID),
                "\"bronze_time_ms\" \"0\"\r\n".into(),
                "\"silver_time_ms\" \"0\"\r\n".into(),
                "\"gold_time_ms\" \"0\"\r\n".into(),
            ];
            let mut recovered = out.clone();
            for line in lines {
                recovered = recovered.replacen(&line, "", 1);
            }
            assert_eq!(recovered, original);
            assert_eq!(original, snapshot);

            let source = fixture(
                vmf,
                "\"metadata_schema_version\" \" 0000 \" // keep\n\"level_uuid\" \"\" // uuid\n\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n",
            );
            let expected = source
                .replace("\" 0000 \"", "\"1\"")
                .replace("\"\" // uuid", &format!("\"{}\" // uuid", UUID));
            assert_eq!(initialize_source(&source, vmf, UUID).unwrap(), expected);
            let snapshot = source.clone();
            let _ = initialize_source(&source, vmf, UUID);
            assert_eq!(source, snapshot);
        }
    }

    #[test]
    fn test_initialize_preserves_valid_authored_values() {
        for vmf in [false, true] {
            for times in [
                [" 060000 ", "045000", "30000"],
                ["2147483647", "2147483646", "2147483645"],
                ["0", "0", "0"],
            ] {
                let props = format!("\"metadata_schema_version\" \"0001\"\n\"level_uuid\" \" 550E8400-E29B-41D4-A716-446655440000 \"\n\"bronze_time_ms\" \"{}\"\n\"silver_time_ms\" \"{}\"\n\"gold_time_ms\" \"{}\"\n", times[0], times[1], times[2]);
                let src = fixture(vmf, &props);
                assert_eq!(
                    initialize_source(&src, vmf, "bad replacement").unwrap(),
                    src
                );
            }
        }
    }

    #[test]
    fn test_initialize_entity_boundaries() {
        for vmf in [false, true] {
            assert!(matches!(
                initialize_source("{\"classname\" \"other\"}", vmf, UUID),
                Err(crate::error::CompileError::NoMetadataEntity)
            ));
            let a = fixture(vmf, "");
            let dup = format!("{}{}", a, a);
            assert!(matches!(
                initialize_source(&dup, vmf, UUID),
                Err(crate::error::CompileError::DuplicateMetadata)
            ));
            let nested = fixture(
                vmf,
                "{\n\"classname\" \"info_level_metadata\"\n\"level_uuid\" \"bad\"\n}\n",
            );
            assert_eq!(
                initialize_source(&nested, vmf, UUID).unwrap(),
                initialized(
                    vmf,
                    "{\n\"classname\" \"info_level_metadata\"\n\"level_uuid\" \"bad\"\n}\n",
                    UUID
                )
            );
            let before_after = format!("{}{{\n\"level_uuid\" \"{}\"\n\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"classname\" \"info_level_metadata\"\n\"metadata_schema_version\" \"1\"\n\"gold_time_ms\" \"0\"\n}}\n", if vmf { "entity\n" } else { "" }, UUID);
            assert_eq!(
                initialize_source(&before_after, vmf, UUID).unwrap(),
                before_after
            );
            let quoted_braces = fixture(vmf, "\"note\" \"{\"\n\"note2\" \"}\"\n");
            assert_eq!(
                initialize_source(&quoted_braces, vmf, UUID).unwrap(),
                initialized(vmf, "\"note\" \"{\"\n\"note2\" \"}\"\n", UUID)
            );
            let roots = format!("{{\n\"classname\" \"other\"\n{{\n\"classname\" \"info_level_metadata\"\n}}\n}}\n{}", fixture(vmf, ""));
            assert!(initialize_source(&roots, vmf, UUID).is_ok());
            for key in [
                "metadata_schema_version",
                "level_uuid",
                "bronze_time_ms",
                "silver_time_ms",
                "gold_time_ms",
            ] {
                let value = if key == "level_uuid" {
                    UUID
                } else if key == "metadata_schema_version" {
                    "1"
                } else {
                    "0"
                };
                let props = format!("\"metadata_schema_version\" \"1\"\n\"level_uuid\" \"{UUID}\"\n\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n\"{key}\" \"{value}\"\n");
                assert!(
                    matches!(
                        initialize_source(&fixture(vmf, &props), vmf, UUID),
                        Err(crate::error::CompileError::InvalidExtendedMetadata(_))
                    ),
                    "duplicate key={key} vmf={vmf}"
                );
            }
            let classes = fixture(vmf, "\"classname\" \"info_level_metadata\"\n");
            assert!(matches!(
                initialize_source(&classes, vmf, UUID),
                Err(crate::error::CompileError::InvalidExtendedMetadata(_))
            ));
            for bad in [
                "{\"classname\" \"info_level_metadata}",
                "{\"classname\" \"info_level_metadata\\",
                "{{",
            ] {
                assert!(initialize_source(bad, false, UUID).is_err());
            }
            assert!(initialize_source("world\n{\n\"classname\" \"info_level_metadata\"\n}\nentity\n{\n\"classname\" \"info_level_metadata\"\n}\n", true, UUID).is_ok());
        }
    }

    #[test]
    fn test_initialize_rejects_partial_and_invalid_targets() {
        for vmf in [false, true] {
            for mask in 1..7 {
                for value in ["0", "positive"] {
                    let names = ["bronze_time_ms", "silver_time_ms", "gold_time_ms"];
                    let values = ["60000", "45000", "30000"];
                    let props = names
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| mask & (1 << i) != 0)
                        .map(|(i, n)| {
                            format!(
                                "\"{}\" \"{}\"\n",
                                n,
                                if value == "0" { "0" } else { values[i] }
                            )
                        })
                        .collect::<String>();
                    let src = fixture(vmf, &props);
                    assert!(
                        matches!(
                            initialize_source(&src, vmf, UUID),
                            Err(crate::error::CompileError::InvalidExtendedMetadata(_))
                        ),
                        "vmf={vmf} partial mask={mask} value={value}"
                    );
                }
            }
            for values in [
                ["0", "0", "0"],
                ["1", "1", "2"],
                ["2", "1", "0"],
                ["0", "1", "2"],
                ["60000", "60000", "30000"],
                ["60000", "30000", "30000"],
                ["40000", "45000", "30000"],
                ["60000", "25000", "30000"],
                ["0", "45000", "30000"],
                ["60000", "0", "30000"],
                ["60000", "45000", "0"],
                ["1e3", "2", "1"],
                ["-1", "2", "1"],
                ["2147483648", "2", "1"],
            ] {
                let props = ["bronze_time_ms", "silver_time_ms", "gold_time_ms"]
                    .iter()
                    .zip(values)
                    .map(|(n, v)| format!("\"{}\" \"{}\"\n", n, v))
                    .collect::<String>();
                let src = fixture(vmf, &props);
                let result = initialize_source(&src, vmf, UUID);
                if values == ["0", "0", "0"] {
                    assert!(result.is_ok());
                } else {
                    assert!(
                        matches!(
                            result,
                            Err(crate::error::CompileError::InvalidExtendedMetadata(_))
                        ),
                        "vmf={vmf} triplet={values:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_initialize_rejects_bad_existing_identity() {
        for vmf in [false, true] {
            for schema in [None, Some("0")] {
                for raw_uuid in [None, Some(""), Some(" \t ")] {
                    let schema_line = schema
                        .map(|v| format!("\"metadata_schema_version\" \"{v}\"\n"))
                        .unwrap_or_default();
                    let uuid_line = raw_uuid
                        .map(|v| format!("\"level_uuid\" \"{v}\"\n"))
                        .unwrap_or_default();
                    let targets = "\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n";
                    let source = fixture(vmf, &format!("{schema_line}{uuid_line}{targets}"));
                    let snapshot = source.clone();
                    let valid_uuid = format!("\"level_uuid\" \"{UUID}\"\n");
                    let valid_schema = "\"metadata_schema_version\" \"1\"\n";
                    let mut expected = fixture(
                        vmf,
                        &format!(
                            "{}{}{targets}",
                            if schema.is_some() { valid_schema } else { "" },
                            if raw_uuid.is_some() { &valid_uuid } else { "" }
                        ),
                    );
                    let inserted = format!(
                        "{}{}",
                        if schema.is_none() { valid_schema } else { "" },
                        if raw_uuid.is_none() { &valid_uuid } else { "" }
                    );
                    expected = expected.replacen(" }\n", &format!("{inserted} }}\n"), 1);
                    let actual = initialize_source(&source, vmf, UUID).unwrap();
                    assert_eq!(
                        actual, expected,
                        "vmf={vmf} schema={schema:?} uuid={raw_uuid:?}"
                    );
                    assert_eq!(source, snapshot);
                    let actual_snapshot = actual.clone();
                    assert_eq!(
                        initialize_source(&actual, vmf, UUID2).unwrap(),
                        actual_snapshot
                    );
                    assert_eq!(actual, actual_snapshot);
                }
            }
            for schema in ["2", "-1", "1.0", "+1", ""] {
                let src = fixture(
                    vmf,
                    &format!("\"metadata_schema_version\" \"{}\"\n", schema),
                );
                assert!(
                    initialize_source(&src, vmf, UUID).is_err(),
                    "schema={schema:?}"
                );
            }
            let src = fixture(vmf, "\"level_uuid\" \"bad\"\n");
            assert!(initialize_source(&src, vmf, UUID).is_err());
            assert!(initialize_source(&fixture(vmf, ""), vmf, "bad").is_err());
            let valid = fixture(vmf, &format!("\"metadata_schema_version\" \"1\"\n\"level_uuid\" \"{}\"\n\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n", UUID));
            assert_eq!(initialize_source(&valid, vmf, "bad").unwrap(), valid);
            let original = fixture(vmf, "\"metadata_schema_version\" \"2\"\n");
            let snapshot = original.clone();
            assert!(initialize_source(&original, vmf, UUID).is_err());
            assert_eq!(original, snapshot);
        }
    }
}
