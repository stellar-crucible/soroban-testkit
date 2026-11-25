//! Deterministic, golden-file snapshots for contract state, events and return values.
//!
//! [`snapshot_json`] converts a Soroban value into canonical JSON: `Val` and `ScVal` are
//! rendered directly, while `#[contracttype]` structs become the sorted map the SDK
//! produces for them. [`assert_snapshot_matches`] stores that text under
//! `tests/snapshots/<name>.snap.json` on the first run and compares later runs against
//! the stored file, reporting a colored unified diff when the value drifts.
//!
//! Two environment variables change the behaviour:
//!
//! - `SNAPSHOT_DIR` overrides the runtime `CARGO_MANIFEST_DIR/tests/snapshots` path.
//! - `UPDATE_SNAPSHOTS=1` rewrites the accepted snapshot instead of failing on drift.

use soroban_sdk::{
    xdr::{
        int128_helpers::{i128_from_pieces, u128_from_pieces},
        ScVal,
    },
    Env, TryFromVal, Val,
};
use std::{
    collections::BTreeMap,
    env,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
};

const DEFAULT_SNAPSHOT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots");
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";
const CONTEXT: usize = 3;
const ADDRESS_KEY: &str = "$address";

/// Controls how a snapshot is normalized before it is stored or compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotOptions {
    normalize: bool,
    ignored_fields: Vec<String>,
}

impl SnapshotOptions {
    /// Options that compare the canonical rendering exactly.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace every address with a stable `~addressN` placeholder, numbered by first
    /// appearance in the document. Use it when generated addresses would otherwise make
    /// an otherwise unchanged snapshot fail.
    pub fn normalize_addresses(mut self, enabled: bool) -> Self {
        self.normalize = enabled;
        self
    }

    /// Drop a field at every depth before comparing or writing the snapshot, for
    /// volatile values such as timestamps or nonces.
    pub fn ignore_field(mut self, field: impl Into<String>) -> Self {
        self.ignored_fields.push(field.into());
        self
    }
}

/// The minimal JSON model used for canonical rendering and comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    fn number(value: impl Display) -> Self {
        Self::Number(value.to_string())
    }
}

fn escape_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_compact(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(boolean) => out.push_str(if *boolean { "true" } else { "false" }),
        Json::Number(number) => out.push_str(number),
        Json::String(text) => escape_string(text, out),
        Json::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        Json::Object(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                escape_string(key, out);
                out.push(':');
                write_compact(item, out);
            }
            out.push('}');
        }
    }
}

fn write_pretty(value: &Json, out: &mut String, depth: usize) {
    match value {
        Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Json::Object(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                out.push_str(&"  ".repeat(depth + 1));
                escape_string(key, out);
                out.push_str(": ");
                write_pretty(item, out, depth + 1);
                if index + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(depth));
            out.push('}');
        }
        Json::Array(items) if items.is_empty() => out.push_str("[]"),
        Json::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&"  ".repeat(depth + 1));
                write_pretty(item, out, depth + 1);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(depth));
            out.push(']');
        }
        _ => write_compact(value, out),
    }
}

fn compact_json(value: &Json) -> String {
    let mut out = String::new();
    write_compact(value, &mut out);
    out
}

fn pretty_json(value: &Json) -> String {
    let mut out = String::new();
    write_pretty(value, &mut out, 0);
    out
}

/// Render an `ScVal` as canonical JSON.
fn scval_to_json(value: &ScVal) -> Json {
    match value {
        ScVal::Void => Json::Null,
        ScVal::Bool(boolean) => Json::Bool(*boolean),
        ScVal::U32(number) => Json::number(*number),
        ScVal::I32(number) => Json::number(*number),
        ScVal::U64(number) => Json::number(*number),
        ScVal::I64(number) => Json::number(*number),
        ScVal::Timepoint(timepoint) => Json::number(timepoint.0),
        ScVal::Duration(duration) => Json::number(duration.0),
        ScVal::U128(parts) => Json::number(u128_from_pieces(parts.hi, parts.lo)),
        ScVal::I128(parts) => Json::number(i128_from_pieces(parts.hi, parts.lo)),
        ScVal::Symbol(symbol) => Json::String(symbol.0.to_string()),
        ScVal::String(text) => Json::String(String::from_utf8_lossy(&text.0.to_vec()).into_owned()),
        ScVal::Bytes(bytes) => {
            let mut text = String::from("0x");
            for byte in bytes.0.iter() {
                text.push_str(&format!("{byte:02x}"));
            }
            Json::String(text)
        }
        ScVal::Address(address) => {
            let mut entry = BTreeMap::new();
            entry.insert(ADDRESS_KEY.to_owned(), Json::String(format!("{address:?}")));
            Json::Object(entry)
        }
        ScVal::Vec(Some(items)) => Json::Array(items.as_vec().iter().map(scval_to_json).collect()),
        ScVal::Vec(None) => Json::Array(Vec::new()),
        ScVal::Map(Some(entries)) => {
            let mut object = BTreeMap::new();
            for entry in entries.as_vec() {
                object.insert(map_key(&entry.key), scval_to_json(&entry.val));
            }
            Json::Object(object)
        }
        ScVal::Map(None) => Json::Object(BTreeMap::new()),
        other => Json::String(format!("{other:?}")),
    }
}

fn map_key(key: &ScVal) -> String {
    match key {
        ScVal::Symbol(symbol) => symbol.0.to_string(),
        other => compact_json(&scval_to_json(other)),
    }
}

fn replace_addresses(value: &mut Json, seen: &mut Vec<String>) {
    match value {
        Json::Object(entries) => {
            if entries.len() == 1 && entries.contains_key(ADDRESS_KEY) {
                if let Some(Json::String(address)) = entries.get(ADDRESS_KEY).cloned() {
                    let index = match seen.iter().position(|item| item == &address) {
                        Some(index) => index,
                        None => {
                            seen.push(address);
                            seen.len() - 1
                        }
                    };
                    entries.insert(
                        ADDRESS_KEY.to_owned(),
                        Json::String(format!("~address{index}")),
                    );
                    return;
                }
            }
            for item in entries.values_mut() {
                replace_addresses(item, seen);
            }
        }
        Json::Array(items) => {
            for item in items {
                replace_addresses(item, seen);
            }
        }
        _ => {}
    }
}

fn remove_field(value: &mut Json, field: &str) {
    match value {
        Json::Object(entries) => {
            entries.remove(field);
            for item in entries.values_mut() {
                remove_field(item, field);
            }
        }
        Json::Array(items) => {
            for item in items {
                remove_field(item, field);
            }
        }
        _ => {}
    }
}

fn normalize_one(value: &mut Json, options: &SnapshotOptions) {
    if options.normalize {
        replace_addresses(value, &mut Vec::new());
    }
    for field in &options.ignored_fields {
        remove_field(value, field);
    }
}

fn normalize_pair(expected: &mut Json, actual: &mut Json, options: &SnapshotOptions) {
    if options.normalize {
        let mut seen = Vec::new();
        replace_addresses(expected, &mut seen);
        replace_addresses(actual, &mut seen);
    }
    for field in &options.ignored_fields {
        remove_field(expected, field);
        remove_field(actual, field);
    }
}

struct JsonParser {
    chars: Vec<char>,
    position: usize,
}

impl JsonParser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            position: 0,
        }
    }

    fn fail(&self, reason: &str) -> ! {
        panic!(
            "invalid snapshot JSON at character {}: {reason}",
            self.position
        )
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += 1;
        Some(character)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\n' | '\r' | '\t')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: char) {
        match self.bump() {
            Some(character) if character == expected => {}
            _ => self.fail(&format!("expected '{expected}'")),
        }
    }

    fn literal(&mut self, word: &str) {
        for expected in word.chars() {
            self.expect(expected);
        }
    }

    fn string(&mut self) -> String {
        self.expect('"');
        let mut out = String::new();
        loop {
            let character = match self.bump() {
                Some(character) => character,
                None => self.fail("unterminated string"),
            };
            match character {
                '"' => return out,
                '\\' => {
                    let escape = match self.bump() {
                        Some(escape) => escape,
                        None => self.fail("unterminated escape"),
                    };
                    match escape {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{08}'),
                        'f' => out.push('\u{0c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                let digit = match self.bump() {
                                    Some(digit) => digit,
                                    None => self.fail("truncated unicode escape"),
                                };
                                let value = match digit.to_digit(16) {
                                    Some(value) => value,
                                    None => self.fail("invalid unicode escape"),
                                };
                                code = code * 16 + value;
                            }
                            match char::from_u32(code) {
                                Some(character) => out.push(character),
                                None => self.fail("invalid unicode escape"),
                            }
                        }
                        _ => self.fail("unknown escape"),
                    }
                }
                character => out.push(character),
            }
        }
    }

    fn number(&mut self) -> Json {
        let start = self.position;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
        {
            self.position += 1;
        }
        if self.position == start {
            self.fail("expected number");
        }
        Json::Number(self.chars[start..self.position].iter().collect())
    }

    fn value(&mut self) -> Json {
        self.skip_whitespace();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Json::String(self.string()),
            Some('t') => {
                self.literal("true");
                Json::Bool(true)
            }
            Some('f') => {
                self.literal("false");
                Json::Bool(false)
            }
            Some('n') => {
                self.literal("null");
                Json::Null
            }
            Some(c) if c.is_ascii_digit() || c == '-' => self.number(),
            _ => self.fail("expected a JSON value"),
        }
    }

    fn object(&mut self) -> Json {
        self.expect('{');
        let mut entries = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.position += 1;
            return Json::Object(entries);
        }
        loop {
            self.skip_whitespace();
            let key = self.string();
            self.skip_whitespace();
            self.expect(':');
            let item = self.value();
            entries.insert(key, item);
            self.skip_whitespace();
            match self.bump() {
                Some(',') => {}
                Some('}') => return Json::Object(entries),
                _ => self.fail("expected ',' or '}'"),
            }
        }
    }

    fn array(&mut self) -> Json {
        self.expect('[');
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.position += 1;
            return Json::Array(items);
        }
        loop {
            items.push(self.value());
            self.skip_whitespace();
            match self.bump() {
                Some(',') => {}
                Some(']') => return Json::Array(items),
                _ => self.fail("expected ',' or ']'"),
            }
        }
    }
}

fn parse_json(input: &str) -> Json {
    let mut parser = JsonParser::new(input);
    let value = parser.value();
    parser.skip_whitespace();
    if parser.peek().is_some() {
        parser.fail("trailing characters");
    }
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOp {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone)]
struct DiffLine {
    op: DiffOp,
    text: String,
    old_number: Option<usize>,
    new_number: Option<usize>,
}

fn line_diff(expected: &[&str], actual: &[&str]) -> Vec<DiffLine> {
    let rows = expected.len();
    let columns = actual.len();
    let mut lengths = vec![vec![0usize; columns + 1]; rows + 1];
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            lengths[row][column] = if expected[row] == actual[column] {
                lengths[row + 1][column + 1] + 1
            } else {
                lengths[row + 1][column].max(lengths[row][column + 1])
            };
        }
    }

    let mut lines = Vec::new();
    let (mut row, mut column) = (0, 0);
    while row < rows || column < columns {
        if row < rows && column < columns && expected[row] == actual[column] {
            lines.push(DiffLine {
                op: DiffOp::Equal,
                text: expected[row].to_owned(),
                old_number: Some(row + 1),
                new_number: Some(column + 1),
            });
            row += 1;
            column += 1;
        } else if column < columns
            && (row == rows || lengths[row][column + 1] >= lengths[row + 1][column])
        {
            lines.push(DiffLine {
                op: DiffOp::Insert,
                text: actual[column].to_owned(),
                old_number: None,
                new_number: Some(column + 1),
            });
            column += 1;
        } else {
            lines.push(DiffLine {
                op: DiffOp::Delete,
                text: expected[row].to_owned(),
                old_number: Some(row + 1),
                new_number: None,
            });
            row += 1;
        }
    }
    lines
}

fn hunk_ranges(lines: &[DiffLine]) -> Vec<(usize, usize)> {
    let changed = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.op != DiffOp::Equal)
        .map(|(index, _)| index);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        match ranges.last_mut() {
            Some(last) if index <= last.1 + CONTEXT * 2 + 1 => last.1 = index,
            _ => ranges.push((index, index)),
        }
    }
    ranges
}

fn hunk_origin(number: Option<usize>) -> usize {
    number.unwrap_or(0)
}

fn format_unified_diff(name: &str, path: &Path, expected: &Json, actual: &Json) -> String {
    let expected_text = pretty_json(expected);
    let actual_text = pretty_json(actual);
    let expected_lines: Vec<&str> = expected_text.lines().collect();
    let actual_lines: Vec<&str> = actual_text.lines().collect();
    let lines = line_diff(&expected_lines, &actual_lines);
    let displayed = path.display();

    let mut out = format!(
        "snapshot assertion failed for \"{name}\": stored snapshot does not match the current value\n\
         {CYAN}--- expected (stored): {displayed}{RESET}\n\
         {CYAN}+++ actual (current): {name}{RESET}\n"
    );
    for (first, last) in hunk_ranges(&lines) {
        let start = first.saturating_sub(CONTEXT);
        let end = (last + CONTEXT + 1).min(lines.len());
        let hunk = &lines[start..end];
        let old_count = hunk.iter().filter(|line| line.old_number.is_some()).count();
        let new_count = hunk.iter().filter(|line| line.new_number.is_some()).count();
        let old_start = if old_count == 0 {
            0
        } else {
            hunk_origin(hunk.iter().find_map(|line| line.old_number).or_else(|| {
                lines[..start]
                    .iter()
                    .filter_map(|line| line.old_number)
                    .next_back()
            }))
        };
        let new_start = if new_count == 0 {
            0
        } else {
            hunk_origin(hunk.iter().find_map(|line| line.new_number).or_else(|| {
                lines[..start]
                    .iter()
                    .filter_map(|line| line.new_number)
                    .next_back()
            }))
        };
        out.push_str(&format!(
            "{CYAN}@@ -{old_start},{old_count} +{new_start},{new_count} @@{RESET}\n"
        ));
        for line in hunk {
            let (prefix, color) = match line.op {
                DiffOp::Equal => (' ', ""),
                DiffOp::Delete => ('-', RED),
                DiffOp::Insert => ('+', GREEN),
            };
            out.push_str(&format!("{color}{prefix}{}{RESET}\n", line.text));
        }
    }
    out.push_str("set UPDATE_SNAPSHOTS=1 to accept the new snapshot");
    out
}

/// Directory in which snapshots are stored. `SNAPSHOT_DIR` wins over the runtime
/// `CARGO_MANIFEST_DIR/tests/snapshots` default.
#[doc(hidden)]
pub fn snapshot_dir() -> PathBuf {
    if let Some(directory) = env::var_os("SNAPSHOT_DIR") {
        return PathBuf::from(directory);
    }
    if let Some(manifest) = env::var_os("CARGO_MANIFEST_DIR") {
        return PathBuf::from(manifest).join("tests/snapshots");
    }
    PathBuf::from(DEFAULT_SNAPSHOT_DIR)
}

/// Path of the snapshot file for `name`.
#[doc(hidden)]
pub fn snapshot_path(name: &str) -> PathBuf {
    let stem: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.is_empty() { "snapshot" } else { &stem };
    snapshot_dir().join(format!("{stem}.snap.json"))
}

fn update_enabled() -> bool {
    env::var("UPDATE_SNAPSHOTS")
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn write_snapshot(path: &Path, value: &Json) {
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            panic!(
                "cannot create snapshot directory {}: {error}",
                parent.display()
            );
        }
    }
    let mut text = pretty_json(value);
    text.push('\n');
    if let Err(error) = fs::write(path, text) {
        panic!("cannot write snapshot {}: {error}", path.display());
    }
}

/// Convert `value` to canonical snapshot JSON, applying normalization options first.
#[doc(hidden)]
pub fn snapshot_json_with_options<V>(env: &Env, value: &V, options: &SnapshotOptions) -> String
where
    Val: TryFromVal<Env, V>,
{
    let val = <Val as TryFromVal<Env, V>>::try_from_val(env, value)
        .unwrap_or_else(|error| panic!("cannot convert value to Val: {error:?}"));
    let scval = <ScVal as TryFromVal<Env, Val>>::try_from_val(env, &val)
        .unwrap_or_else(|error| panic!("cannot convert Val to ScVal: {error:?}"));
    let mut json = scval_to_json(&scval);
    normalize_one(&mut json, options);
    pretty_json(&json)
}

/// Render a `Val`, an `ScVal`, or a `#[contracttype]` value as canonical JSON.
///
/// Map keys are sorted, strings use one stable escaping scheme, and integers are
/// rendered as decimal text. The result is independent of the order in which a
/// contract inserted map entries.
pub fn snapshot_json<V>(env: &Env, value: &V) -> String
where
    Val: TryFromVal<Env, V>,
{
    snapshot_json_with_options(env, value, &SnapshotOptions::new())
}

/// Compare JSON with the stored snapshot for `name`, creating or updating it as needed.
#[doc(hidden)]
pub fn assert_snapshot_matches_impl(name: &str, json: &str, options: &SnapshotOptions) {
    let path = snapshot_path(name);
    let mut actual = parse_json(json);
    if !path.exists() {
        write_snapshot(&path, &actual);
        return;
    }
    let stored = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read snapshot {}: {error}", path.display()));
    let mut expected = parse_json(&stored);
    normalize_pair(&mut expected, &mut actual, options);
    if compact_json(&expected) == compact_json(&actual) {
        return;
    }
    if update_enabled() {
        write_snapshot(&path, &actual);
        return;
    }
    panic!("{}", format_unified_diff(name, &path, &expected, &actual));
}

/// Assert that canonical JSON matches the committed snapshot.
///
/// Missing snapshots are created automatically. On drift the failure is a colored
/// unified diff with line numbers; set `UPDATE_SNAPSHOTS=1` to accept the new value.
pub fn assert_snapshot_matches(name: &str, json: &str) {
    assert_snapshot_matches_impl(name, json, &SnapshotOptions::new());
}

/// Compare JSON with the stored snapshot using normalization options.
#[doc(hidden)]
pub fn assert_snapshot_matches_with_options(name: &str, json: &str, options: &SnapshotOptions) {
    assert_snapshot_matches_impl(name, json, options);
}

/// Environment used by the two-argument macro arm for values that do not belong to a
/// caller-owned `Env`.
#[doc(hidden)]
pub fn default_env() -> Env {
    Env::default()
}

/// Snapshot assertion with an optional `Env` and [`SnapshotOptions`].
///
/// `macro_rules!` cannot capture an implicit caller-local identifier, so the two-argument
/// arm uses a fresh `Env` and is intended for values that do not reference host objects.
/// Pass the environment explicitly for `Val`, `Address`, `Map` or `#[contracttype]` values.
///
/// ```ignore
/// assert_snapshot_matches!("answer", 42u32);
/// assert_snapshot_matches!("state", state, env);
///
/// // Volatile fields and generated addresses can be normalized:
/// let options = SnapshotOptions::new()
///     .normalize_addresses(true)
///     .ignore_field("updated_at");
/// assert_snapshot_matches!("state", state, env, options);
/// ```
#[macro_export]
macro_rules! assert_snapshot_matches {
    ($name:expr, $value:expr) => {{
        let env = $crate::default_env();
        $crate::assert_snapshot_matches_impl(
            $name,
            &$crate::snapshot_json(&env, &$value),
            &$crate::SnapshotOptions::new(),
        )
    }};
    ($name:expr, $value:expr, $env:expr) => {
        $crate::assert_snapshot_matches_impl(
            $name,
            &$crate::snapshot_json(&$env, &$value),
            &$crate::SnapshotOptions::new(),
        )
    };
    ($name:expr, $value:expr, $env:expr, $options:expr) => {{
        let options = $options;
        $crate::assert_snapshot_matches_with_options(
            $name,
            &$crate::snapshot_json_with_options(&$env, &$value, &options),
            &options,
        )
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_and_escapes_strings() {
        let json = parse_json("{\"b\": 2, \"a\": \"x\\n\\\"y\"}");
        assert_eq!(compact_json(&json), "{\"a\":\"x\\n\\\"y\",\"b\":2}");
        assert_eq!(parse_json(&compact_json(&json)), json);
    }

    #[test]
    fn unified_diff_uses_exact_line_numbers() {
        let expected = parse_json("{\"a\": 1, \"b\": 2}");
        let actual = parse_json("{\"a\": 9, \"b\": 2}");
        let message =
            format_unified_diff("name", Path::new("snapshot.snap.json"), &expected, &actual);
        assert!(message.contains("@@ -1,4 +1,4 @@"), "{message}");
        assert!(message.contains("\x1b[31m-  \"a\": 1,\x1b[0m"), "{message}");
        assert!(message.contains("\x1b[32m+  \"a\": 9,\x1b[0m"), "{message}");
        assert!(message.contains("\x1b[36m"), "{message}");
    }

    #[test]
    fn options_remove_fields_recursively() {
        let mut json = parse_json("{\"skip\": 1, \"nested\": [{\"keep\": 2, \"skip\": 3}]}");
        let options = SnapshotOptions::new().ignore_field("skip");
        normalize_one(&mut json, &options);
        assert_eq!(compact_json(&json), "{\"nested\":[{\"keep\":2}]}");
    }

    #[test]
    fn address_placeholders_follow_first_appearance() {
        let options = SnapshotOptions::new().normalize_addresses(true);
        let mut first = parse_json(r#"{"a":{"$address":"ACC_A"},"b":{"$address":"ACC_B"}}"#);
        let mut second = parse_json(r#"{"a":{"$address":"ACC_C"},"b":{"$address":"ACC_D"}}"#);
        normalize_one(&mut first, &options);
        normalize_one(&mut second, &options);
        assert_eq!(compact_json(&first), compact_json(&second));
        assert!(compact_json(&first).contains("~address0"));
        assert!(compact_json(&first).contains("~address1"));
    }
}
