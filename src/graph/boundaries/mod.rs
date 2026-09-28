//! Dynamic-dispatch boundaries: where a symbol's NAME appears in a shape the
//! static graph does not turn into an edge.
//!
//! When `callgraph` / `impact` / `refs` (and their MCP twins) find no caller for
//! a function, "nobody calls it" and "it is called through a string key, a
//! reflection primitive, an event name or a function value" look identical.
//! This module tells them apart by DISCLOSURE only: it never adds an edge, and
//! it runs only on an empty caller result, so a non-empty answer is unchanged.
//!
//! # Accepted shapes (one site per line; the first matching kind wins)
//!
//! | kind                 | example                                            |
//! |----------------------|----------------------------------------------------|
//! | `reflection`         | `getattr(o, "save")`, `send(:save)`, `getMethod("save")`, `dlsym(h, "save")` |
//! | `event_name`         | `bus.on("save", f)`, `emit('save')`, `ipcRenderer.invoke("save")` |
//! | `string_key`         | `handlers["save"]`, `{"save": f}`, `'save' => …`   |
//! | `symbol`             | Ruby `before_action :save`                         |
//! | `function_reference` | `register(save)`, `.map(Self::save)`, `{ save, load }`, `x = save` |
//!
//! Look-alikes deliberately NOT reported: comments; strings that merely
//! mention the name (`log("save")`, `"save failed"`); identifiers containing
//! it (`autosave`, `save_all`, `$save`); the definition; a direct call
//! `save(…)` (a static edge — or a resolver gap, which is not dispatch);
//! imports / re-exports / destructuring; parameters; conditions
//! (`if (x.save)`); ternaries and `case "save":`. Ruby has no
//! function-reference shape (a bare `save` there IS a call). The corpus in
//! `tests.rs` pins every row of this table.
//!
//! Comments and string contents are blanked with byte offsets preserved, so
//! a shape is matched against code only — except where the string IS the key
//! (`handlers["save"]`, `getattr(o, "save")`), which is read from the literal.
//!
//! Test files are not scanned (a spec's `receive(:save)` is not production
//! dispatch), and neither are languages without a shape table (markdown,
//! json, html, css, bash).

use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

/// Sites listed per answer; the rest are counted.
pub const BOUNDARY_SITE_CAP: usize = 5;

/// Files larger than this are skipped (minified bundles, generated tables).
const MAX_SCAN_BYTES: u64 = 2 * 1024 * 1024;

/// Kind of dynamic-dispatch shape. Declaration order is priority order: when
/// one line carries several shapes, the smallest wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    Reflection,
    EventName,
    StringKey,
    Symbol,
    FunctionReference,
}

impl Shape {
    /// Machine name (JSON `shape`).
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Reflection => "reflection",
            Shape::EventName => "event_name",
            Shape::StringKey => "string_key",
            Shape::Symbol => "symbol",
            Shape::FunctionReference => "function_reference",
        }
    }

    /// Human label (text output).
    pub fn label(self) -> &'static str {
        match self {
            Shape::Reflection => "reflection",
            Shape::EventName => "event name",
            Shape::StringKey => "string key",
            Shape::Symbol => "symbol",
            Shape::FunctionReference => "function reference",
        }
    }
}

/// One shape found in one source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeHit {
    /// 1-based line.
    pub line: usize,
    pub shape: Shape,
    /// The dispatching callee for `reflection` / `event_name` (`getattr`, `on`).
    pub via: Option<String>,
}

/// Callees that look a member up by name.
const REFLECTION_CALLEES: &[&str] = &[
    "getattr",
    "hasattr",
    "methodcaller",
    "send",
    "public_send",
    "__send__",
    "method",
    "instance_method",
    "public_method",
    "respond_to?",
    "getMethod",
    "getDeclaredMethod",
    "MethodByName",
    "GetMethod",
    "InvokeMember",
    "invokeMethod",
    "dlsym",
    "GetProcAddress",
    "call_user_func",
    "call_user_func_array",
    "method_exists",
    "is_callable",
];

/// Callees that route by an event / channel / command name.
const EVENT_CALLEES: &[&str] = &[
    "on",
    "once",
    "off",
    "emit",
    "addListener",
    "prependListener",
    "removeListener",
    "addEventListener",
    "removeEventListener",
    "subscribe",
    "publish",
    "dispatch",
    "trigger",
    "handle",
    "invoke",
    "$on",
    "$emit",
];

/// A call whose parenthesized argument is a condition, not a value.
const CONTROL_KEYWORDS: &[&str] = &[
    "if", "while", "for", "switch", "match", "elif", "catch", "until", "unless",
];

/// A call-looking `(` that opens a parameter list.
const DEF_KEYWORDS: &[&str] = &["def", "fn", "function", "func", "fun"];

#[derive(Clone, Copy)]
enum SingleQuote {
    /// `'…'` is a string (JS, Python, Ruby, PHP, Dart).
    Str,
    /// `'x'` is a char literal (C, Java, Go, C#, Kotlin, Swift).
    Char,
    /// `'x'` is a char, `'a` a lifetime (Rust).
    RustChar,
}

#[derive(Clone, Copy)]
struct Syntax {
    slash_comments: bool,
    hash_comments: bool,
    nested_block_comments: bool,
    single_quote: SingleQuote,
    /// Backtick string (JS template, Go raw string).
    backtick: bool,
    /// `${…}` interpolation inside backtick strings (JS/TS).
    template_interp: bool,
    triple_quotes: bool,
    rust_raw: bool,
    /// `"…"` may span lines (Rust, Ruby, PHP).
    multiline_dquote: bool,
    ruby: bool,
    /// `$` is an identifier character (JS/TS, Dart, PHP variables).
    dollar_ident: bool,
    /// `"save": …` is a table key. Not Rust, where it only occurs as `json!`
    /// data.
    key_colon: bool,
    /// `"save" => …` is a table key (Ruby / PHP hash rocket; in Rust and Scala
    /// `=>` is a match arm).
    key_rocket: bool,
    /// `x["save"] = …` stores data, never a handler (Rust: `HashMap` has no
    /// `IndexMut`, so a subscript assignment is always `serde_json`).
    index_store_is_data: bool,
    /// `if (…) {` — conditions are parenthesized, so `name(…) {` opens a
    /// method body (JS/TS, Java, C/C++, C#, Kotlin, Dart, PHP).
    paren_conditions: bool,
    /// `save.bind(…)` / `.call(…)` / `.apply(…)` hands the function on (JS/TS).
    function_methods: bool,
}

fn syntax_for(language: &str) -> Option<Syntax> {
    let base = Syntax {
        slash_comments: true,
        hash_comments: false,
        nested_block_comments: false,
        single_quote: SingleQuote::Char,
        backtick: false,
        template_interp: false,
        triple_quotes: false,
        rust_raw: false,
        multiline_dquote: false,
        ruby: false,
        dollar_ident: false,
        key_colon: true,
        key_rocket: false,
        index_store_is_data: false,
        paren_conditions: true,
        function_methods: false,
    };
    Some(match language {
        "javascript" | "typescript" | "tsx" => Syntax {
            single_quote: SingleQuote::Str,
            backtick: true,
            template_interp: true,
            dollar_ident: true,
            function_methods: true,
            ..base
        },
        "go" => Syntax {
            backtick: true,
            paren_conditions: false,
            ..base
        },
        "rust" => Syntax {
            nested_block_comments: true,
            single_quote: SingleQuote::RustChar,
            rust_raw: true,
            multiline_dquote: true,
            key_colon: false,
            index_store_is_data: true,
            paren_conditions: false,
            ..base
        },
        "java" | "c" | "cpp" | "csharp" => base,
        "kotlin" => Syntax {
            nested_block_comments: true,
            triple_quotes: true,
            ..base
        },
        "swift" => Syntax {
            nested_block_comments: true,
            triple_quotes: true,
            paren_conditions: false,
            ..base
        },
        "dart" => Syntax {
            nested_block_comments: true,
            single_quote: SingleQuote::Str,
            triple_quotes: true,
            dollar_ident: true,
            ..base
        },
        "php" => Syntax {
            hash_comments: true,
            single_quote: SingleQuote::Str,
            multiline_dquote: true,
            dollar_ident: true,
            key_rocket: true,
            ..base
        },
        "python" => Syntax {
            slash_comments: false,
            hash_comments: true,
            single_quote: SingleQuote::Str,
            triple_quotes: true,
            paren_conditions: false,
            ..base
        },
        "ruby" => Syntax {
            slash_comments: false,
            hash_comments: true,
            single_quote: SingleQuote::Str,
            multiline_dquote: true,
            ruby: true,
            key_rocket: true,
            paren_conditions: false,
            ..base
        },
        _ => return None,
    })
}

/// A string literal: quote positions in the source and its content, when the
/// content is static (no interpolation).
struct StrLit {
    open: usize,
    close: usize,
    content: Option<String>,
}

/// Source with comments and string contents blanked to spaces (newlines kept,
/// offsets preserved), plus the string literals found.
struct Lexed {
    masked: Vec<u8>,
    strings: Vec<StrLit>,
}

fn is_ident_byte(b: u8, syn: &Syntax) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 || (syn.dollar_ident && b == b'$')
}

fn blank(masked: &mut [u8], from: usize, to: usize) {
    for b in &mut masked[from..to] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

/// Byte length of the UTF-8 char starting at `i`.
fn char_len(src: &[u8], i: usize) -> usize {
    match src.get(i) {
        Some(b) if *b < 0x80 => 1,
        Some(b) if *b >= 0xF0 => 4,
        Some(b) if *b >= 0xE0 => 3,
        Some(_) => 2,
        None => 0,
    }
}

fn lex(src: &[u8], syn: &Syntax) -> Lexed {
    let mut masked = src.to_vec();
    let mut strings = Vec::new();
    let n = src.len();
    let mut i = 0;
    // Brace depth of each open `${` (JS template interpolation).
    let mut interp: Vec<usize> = Vec::new();
    let at_line_start = |i: usize| i == 0 || src[i - 1] == b'\n';
    while i < n {
        let c = src[i];
        // Resume a template literal when its `${…}` closes.
        if !interp.is_empty() {
            if c == b'{' {
                *interp.last_mut().unwrap() += 1;
            } else if c == b'}' {
                let top = interp.last_mut().unwrap();
                if *top == 0 {
                    interp.pop();
                    i = scan_template(src, &mut masked, i + 1, &mut interp, &mut strings, None);
                    continue;
                }
                *top -= 1;
            }
        }
        // Comments.
        if syn.slash_comments && c == b'/' && i + 1 < n {
            if src[i + 1] == b'/' {
                let end = src[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(n, |p| i + p);
                blank(&mut masked, i, end);
                i = end;
                continue;
            }
            if src[i + 1] == b'*' {
                let mut depth = 1usize;
                let mut j = i + 2;
                while j < n && depth > 0 {
                    if syn.nested_block_comments && src[j] == b'/' && src.get(j + 1) == Some(&b'*')
                    {
                        depth += 1;
                        j += 2;
                    } else if src[j] == b'*' && src.get(j + 1) == Some(&b'/') {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut masked, i, j);
                i = j;
                continue;
            }
        }
        if syn.hash_comments && c == b'#' {
            let end = src[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(n, |p| i + p);
            blank(&mut masked, i, end);
            i = end;
            continue;
        }
        if syn.ruby && c == b'=' && at_line_start(i) && src[i..].starts_with(b"=begin") {
            let end = find_sub(src, i, b"\n=end").map_or(n, |p| {
                src[p + 1..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(n, |q| p + 1 + q)
            });
            blank(&mut masked, i, end);
            i = end;
            continue;
        }
        // Rust raw strings: r"…", r#"…"#, br"…".
        if syn.rust_raw
            && c == b'r'
            && (i == 0 || !is_ident_byte(src[i - 1], syn) || src[i - 1] == b'b')
        {
            let mut j = i + 1;
            while j < n && src[j] == b'#' {
                j += 1;
            }
            let hashes = j - i - 1;
            if j < n && src[j] == b'"' {
                let mut term = vec![b'"'];
                term.extend(std::iter::repeat_n(b'#', hashes));
                let close = find_sub(src, j + 1, &term).unwrap_or(n);
                record(src, &mut masked, &mut strings, j, close, true);
                i = (close + term.len()).min(n);
                continue;
            }
        }
        // Triple-quoted strings.
        if syn.triple_quotes && (c == b'"' || c == b'\'') && src[i..].starts_with(&[c, c, c]) {
            let close = find_sub(src, i + 3, &[c, c, c]).unwrap_or(n);
            blank(&mut masked, i + 3, close);
            strings.push(StrLit {
                open: i,
                close,
                content: None,
            });
            i = (close + 3).min(n);
            continue;
        }
        if c == b'"' {
            let close = scan_quoted(src, i + 1, b'"', syn.multiline_dquote);
            record(src, &mut masked, &mut strings, i, close, false);
            i = (close + 1).min(n);
            continue;
        }
        if c == b'\'' {
            match syn.single_quote {
                SingleQuote::Str => {
                    let close = scan_quoted(src, i + 1, b'\'', syn.multiline_dquote);
                    record(src, &mut masked, &mut strings, i, close, false);
                    i = (close + 1).min(n);
                    continue;
                }
                SingleQuote::Char => {
                    let close = scan_quoted(src, i + 1, b'\'', false);
                    blank(&mut masked, i + 1, close);
                    i = (close + 1).min(n);
                    continue;
                }
                SingleQuote::RustChar => {
                    // `'\…'` or `'x'` is a char; anything else (`'a`, `'outer:`)
                    // is a lifetime or label and stays code.
                    let is_char = match src.get(i + 1) {
                        Some(b'\\') => true,
                        Some(_) => src.get(i + 1 + char_len(src, i + 1)) == Some(&b'\''),
                        None => false,
                    };
                    if is_char {
                        let close = scan_quoted(src, i + 1, b'\'', false);
                        blank(&mut masked, i + 1, close);
                        i = (close + 1).min(n);
                        continue;
                    }
                }
            }
        }
        if syn.backtick && c == b'`' {
            if syn.template_interp {
                i = scan_template(src, &mut masked, i + 1, &mut interp, &mut strings, Some(i));
            } else {
                let close = src[i + 1..]
                    .iter()
                    .position(|&b| b == b'`')
                    .map_or(n, |p| i + 1 + p);
                record(src, &mut masked, &mut strings, i, close, true);
                i = (close + 1).min(n);
            }
            continue;
        }
        i += 1;
    }
    Lexed { masked, strings }
}

/// Scan a JS template body from `from`; returns the index after the closing
/// backtick, or after a `${` (pushing an interpolation frame). `open` is the
/// opening backtick when this is the literal's first segment — a literal with
/// an interpolation has no static content.
fn scan_template(
    src: &[u8],
    masked: &mut [u8],
    from: usize,
    interp: &mut Vec<usize>,
    strings: &mut Vec<StrLit>,
    open: Option<usize>,
) -> usize {
    let n = src.len();
    let mut j = from;
    while j < n {
        match src[j] {
            b'\\' => j += 2,
            b'`' => {
                blank(masked, from, j);
                if let Some(o) = open {
                    strings.push(StrLit {
                        open: o,
                        close: j,
                        content: std::str::from_utf8(&src[from..j]).ok().map(str::to_string),
                    });
                }
                return j + 1;
            }
            b'$' if src.get(j + 1) == Some(&b'{') => {
                blank(masked, from, j);
                interp.push(0);
                return j + 2;
            }
            _ => j += 1,
        }
    }
    blank(masked, from, n);
    n
}

/// Index of the closing quote (or of the newline / end that stops an
/// unterminated single-line literal).
fn scan_quoted(src: &[u8], from: usize, quote: u8, multiline: bool) -> usize {
    let n = src.len();
    let mut j = from;
    while j < n {
        let b = src[j];
        if b == b'\\' {
            j += 2;
            continue;
        }
        if b == quote || (!multiline && b == b'\n') {
            return j;
        }
        j += 1;
    }
    n
}

fn record(
    src: &[u8],
    masked: &mut [u8],
    strings: &mut Vec<StrLit>,
    open: usize,
    close: usize,
    raw: bool,
) {
    let close = close.min(src.len());
    let body = &src[open + 1..close];
    blank(masked, open + 1, close);
    let content = if !raw && body.contains(&b'\\') {
        None
    } else {
        std::str::from_utf8(body).ok().map(str::to_string)
    };
    strings.push(StrLit {
        open,
        close,
        content,
    });
}

fn find_sub(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Previous non-blank byte strictly before `i`: (index, byte).
fn prev_non_ws(m: &[u8], i: usize) -> Option<(usize, u8)> {
    let mut j = i;
    while j > 0 {
        j -= 1;
        if !m[j].is_ascii_whitespace() {
            return Some((j, m[j]));
        }
    }
    None
}

/// Next non-blank byte at or after `i`: (index, byte, crossed a newline).
fn next_non_ws(m: &[u8], i: usize) -> Option<(usize, u8, bool)> {
    let mut nl = false;
    for (j, &b) in m.iter().enumerate().skip(i) {
        if b == b'\n' {
            nl = true;
        } else if !b.is_ascii_whitespace() {
            return Some((j, b, nl));
        }
    }
    None
}

/// Identifier ending at `end` (exclusive) — Ruby's `?`/`!` suffix included.
fn ident_before(m: &[u8], end: usize, syn: &Syntax) -> (usize, String) {
    let mut s = end;
    if syn.ruby && s > 0 && matches!(m[s - 1], b'?' | b'!') {
        s -= 1;
    }
    while s > 0 && is_ident_byte(m[s - 1], syn) {
        s -= 1;
    }
    (s, String::from_utf8_lossy(&m[s..end]).into_owned())
}

/// The unmatched opener enclosing `pos`, scanning back at most 4 KB.
fn enclosing_opener(m: &[u8], pos: usize) -> Option<usize> {
    let mut depth = 0usize;
    let floor = pos.saturating_sub(4096);
    let mut j = pos;
    while j > floor {
        j -= 1;
        match m[j] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                if depth == 0 {
                    return Some(j);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Name of the call whose argument list encloses `pos`: the nearest `(`,
/// looking through at most one array/object literal (`f([$this, 'save'])`).
fn enclosing_callee(m: &[u8], pos: usize, syn: &Syntax) -> Option<String> {
    let mut at = pos;
    for _ in 0..2 {
        let o = enclosing_opener(m, at)?;
        if m[o] == b'(' {
            let (_, name) =
                prev_non_ws(m, o).map_or((0, String::new()), |(k, _)| ident_before(m, k + 1, syn));
            return (!name.is_empty()).then_some(name);
        }
        at = o;
    }
    None
}

/// Ruby paren-less call: `send :save` / `send "save"` — the identifier just
/// before `pos` on the same line, separated by spaces only.
fn parenless_callee(m: &[u8], pos: usize, syn: &Syntax) -> Option<String> {
    let mut j = pos;
    let mut spaces = 0;
    while j > 0 && (m[j - 1] == b' ' || m[j - 1] == b'\t') {
        j -= 1;
        spaces += 1;
    }
    if spaces == 0 {
        return None;
    }
    let (_, name) = ident_before(m, j, syn);
    (!name.is_empty()).then_some(name)
}

fn classify_callee(name: &str) -> Option<Shape> {
    if REFLECTION_CALLEES.contains(&name) {
        Some(Shape::Reflection)
    } else if EVENT_CALLEES.contains(&name) {
        Some(Shape::EventName)
    } else {
        None
    }
}

/// Text of the line containing `pos`, trimmed.
fn line_of(m: &[u8], pos: usize) -> &[u8] {
    let s = m[..pos]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let e = m[pos..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(m.len(), |p| pos + p);
    m[s..e].trim_ascii()
}

fn is_import_line(line: &[u8]) -> bool {
    const HEADS: &[&str] = &[
        "import ",
        "import{",
        "from ",
        "use ",
        "pub use ",
        "pub(crate) use ",
        "pub(super) use ",
        "#include",
        "using ",
        "extern crate",
        "package ",
        "export {",
        "export{",
        "export *",
        "export type {",
        "export default",
    ];
    HEADS.iter().any(|h| line.starts_with(h.as_bytes()))
}

fn word_before(m: &[u8], pos: usize, syn: &Syntax) -> Option<String> {
    let (k, _) = prev_non_ws(m, pos)?;
    let (_, w) = ident_before(m, k + 1, syn);
    (!w.is_empty()).then_some(w)
}

/// Is the identifier at `[s, e)` written without a qualifier (`save`, not
/// `obj.save` / `Store::save` / `$this->save`)?
fn is_bare(m: &[u8], s: usize) -> bool {
    match prev_non_ws(m, s) {
        Some((p, b'.')) => p > 0 && m[p - 1] == b'.',
        Some((p, b':')) => !(p > 0 && m[p - 1] == b':'),
        Some((p, b'>')) => !(p > 0 && m[p - 1] == b'-'),
        _ => true,
    }
}

/// Does the identifier at `[s, e)` DECLARE a local of that name — a
/// variable, a parameter, a loop variable, a pattern binding? A file that
/// binds the name locally uses the bare name for the local, so its bare
/// references are not the function's (`const url = …; fetch(url)`).
fn is_binding(m: &[u8], s: usize, e: usize, syn: &Syntax) -> bool {
    if !is_bare(m, s) {
        return false;
    }
    // `run = save` binds `run`; `save` is the value.
    if let Some((p, b'=')) = prev_non_ws(m, s) {
        if !(p > 0 && matches!(m[p - 1], b'=' | b'!' | b'<' | b'>')) {
            return false;
        }
    }
    if let Some(w) = word_before(m, s, syn) {
        if matches!(
            w.as_str(),
            "const" | "let" | "var" | "val" | "mut" | "for" | "as" | "lambda"
        ) {
            return true;
        }
    }
    let rest = m[e..].trim_ascii_start();
    // Go `url := …`.
    if rest.starts_with(b":=") {
        return true;
    }
    // Statement-start assignment `url = …` (Python, Ruby, Go, JS re-binding).
    if rest.starts_with(b"=") && !rest.starts_with(b"==") && !rest.starts_with(b"=>") {
        match prev_non_ws(m, s) {
            None => return true,
            Some((p, b)) => {
                let newline_between = m[p + 1..s].contains(&b'\n');
                if newline_between || matches!(b, b';' | b'{' | b'}') {
                    return true;
                }
            }
        }
    }
    // Closure parameters `|url|`, `|url, b|`, `|url: T|`, `|a, url|` — not a
    // bitwise `a | url`.
    let prev = prev_non_ws(m, s);
    let single_bar = matches!(prev, Some((p, b'|')) if p == 0 || m[p - 1] != b'|');
    if (single_bar && (rest.starts_with(b"|") || rest.starts_with(b",") || rest.starts_with(b":")))
        || (matches!(prev, Some((_, b','))) && rest.starts_with(b"|"))
    {
        return true;
    }
    // Inside a parameter list or a destructuring pattern.
    let Some(o) = enclosing_opener(m, s) else {
        return false;
    };
    let after_close = matching_close(m, o).map(|c| m[c + 1..].trim_ascii_start());
    if let Some(r) = after_close {
        if r.starts_with(b"=>") || (r.starts_with(b"=") && !r.starts_with(b"==")) {
            return true;
        }
    }
    match m[o] {
        b'(' => {
            let callee = word_before(m, o, syn);
            if callee
                .as_deref()
                .is_some_and(|c| CONTROL_KEYWORDS.contains(&c))
            {
                return false;
            }
            if callee.as_deref().is_some_and(|c| DEF_KEYWORDS.contains(&c)) {
                return true;
            }
            if let Some((k, _)) = prev_non_ws(m, o) {
                let (ws, _) = ident_before(m, k + 1, syn);
                if word_before(m, ws, syn).is_some_and(|kw| DEF_KEYWORDS.contains(&kw.as_str())) {
                    return true;
                }
            }
            // `m(url) {` / `void f(String url) {` — a method's parameters, in
            // languages whose conditions are parenthesized (in Rust / Go /
            // Swift `if check(url) {` is a call inside a condition).
            syn.paren_conditions && after_close.is_some_and(|r| r.starts_with(b"{"))
        }
        b'{' => {
            word_before(m, o, syn).is_some_and(|w| matches!(w.as_str(), "const" | "let" | "var"))
        }
        _ => false,
    }
}

/// Is the identifier at `[s, e)` a function used as a value?
fn is_function_reference(m: &[u8], s: usize, e: usize, syn: &Syntax) -> bool {
    // `logerror.bind(this)`, `handler.call(ctx)`: the function object itself
    // is used, whatever surrounds it.
    if syn.function_methods
        && [&b".bind("[..], b".call(", b".apply("]
            .iter()
            .any(|t| m[e..].starts_with(t))
    {
        return !is_import_line(line_of(m, s));
    }
    // What follows: a value ends at a separator; `(` is a call, `=` an
    // assignment target, `:` a key / annotation, `.` a property access.
    let value_ends = match next_non_ws(m, e) {
        None => true,
        Some((_, b, false)) => matches!(b, b',' | b')' | b']' | b'}' | b';'),
        Some((_, b, true)) => is_ident_byte(b, syn) || matches!(b, b')' | b']' | b'}'),
    };
    if !value_ends {
        return false;
    }
    // Walk back over a qualifier chain: `a.b.save`, `Self::save`, `$this->save`, `::save`.
    let mut q = s;
    while let Some((p, b)) = prev_non_ws(m, q) {
        let sep_start = match b {
            b'.' if p == 0 || m[p - 1] != b'.' => p,
            b':' if p > 0 && m[p - 1] == b':' => p - 1,
            b'>' if p > 0 && m[p - 1] == b'-' => p - 1,
            _ => break,
        };
        let qual = Syntax {
            dollar_ident: true,
            ..*syn
        };
        let (start, word) = ident_before(m, sep_start, &qual);
        if word.is_empty() {
            if b == b':' {
                // Kotlin/C++ `::save` — the chain starts at the separator.
                q = sep_start;
                break;
            }
            return false;
        }
        q = start;
    }
    let Some((p, pb)) = prev_non_ws(m, q) else {
        return false;
    };
    let ok = match pb {
        b'(' | b'[' | b'{' | b',' => true,
        b'=' => !(p > 0 && matches!(m[p - 1], b'=' | b'!' | b'<' | b'>')),
        b':' => !(p > 0 && m[p - 1] == b':'),
        b'&' => !(p > 0 && m[p - 1] == b'&'),
        _ => word_before(m, q, syn).as_deref() == Some("return"),
    };
    if !ok {
        return false;
    }
    if is_import_line(line_of(m, s)) {
        return false;
    }
    // The right-hand side of `=` is a value wherever it sits — including a
    // default parameter value (`function f(run = save)`, `{ install = save }`),
    // which the parameter-list and pattern rules below would otherwise reject.
    if pb == b'=' {
        return true;
    }
    // Enclosing brackets: parameter lists, conditions, imports, destructuring.
    let mut at = q;
    for _ in 0..3 {
        let Some(o) = enclosing_opener(m, at) else {
            break;
        };
        if is_import_line(line_of(m, o)) {
            return false;
        }
        // A bracket followed by `=` or `=>` is a pattern or a parameter list:
        // `let Some(save) = x`, `[a, save] = f()`, `(a, save) => a`,
        // `Some(save) => …`.
        if let Some(close) = matching_close(m, o) {
            let rest = m[close + 1..].trim_ascii_start();
            if rest.starts_with(b"=>") || (rest.starts_with(b"=") && !rest.starts_with(b"==")) {
                return false;
            }
        }
        match m[o] {
            b'(' => {
                let callee = word_before(m, o, syn);
                if let Some(c) = callee.as_deref() {
                    if CONTROL_KEYWORDS.contains(&c) || DEF_KEYWORDS.contains(&c) {
                        return false;
                    }
                    // `def name(`, `fn name(`, `function name(`.
                    if let Some((k, _)) = prev_non_ws(m, o) {
                        let (ws, _) = ident_before(m, k + 1, syn);
                        if let Some(kw) = word_before(m, ws, syn) {
                            if DEF_KEYWORDS.contains(&kw.as_str()) {
                                return false;
                            }
                        }
                    }
                }
                // Stop at the innermost call: outer brackets belong to other expressions.
                break;
            }
            b'{' => {
                // `${save}` renders the value into text; nothing dispatches it.
                if o > 0 && m[o - 1] == b'$' {
                    return false;
                }
                if let Some(w) = word_before(m, o, syn) {
                    if matches!(w.as_str(), "const" | "let" | "var") {
                        return false;
                    }
                }
            }
            _ => {}
        }
        at = o;
    }
    true
}

fn matching_close(m: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &b) in m.iter().enumerate().skip(open) {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// Does a string literal `content` name `name` — exactly, or as the last
/// segment of a dotted / `::` path (`"app.tasks.save"`)?
fn names(content: &str, name: &str) -> bool {
    if content == name {
        return true;
    }
    content.strip_suffix(name).is_some_and(|head| {
        (head.ends_with('.') || head.ends_with("::") || head.ends_with('#'))
            && head
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '#' | '$'))
    })
}

fn classify_string(
    m: &[u8],
    lit: &StrLit,
    name: &str,
    syn: &Syntax,
) -> Option<(Shape, Option<String>)> {
    let content = lit.content.as_deref()?;
    if !names(content, name) {
        return None;
    }
    let callee = enclosing_callee(m, lit.open, syn).or_else(|| {
        syn.ruby
            .then(|| parenless_callee(m, lit.open, syn))
            .flatten()
    });
    if let Some(c) = callee {
        if let Some(shape) = classify_callee(&c) {
            return Some((shape, Some(c)));
        }
    }
    if content != name {
        return None;
    }
    let before = prev_non_ws(m, lit.open);
    let after = next_non_ws(m, lit.close + 1).map(|(j, b, _)| (j, b));
    // `x["save"]` — except a Rust assignment target: `HashMap` has no
    // `IndexMut`, so `out["save"] = …` there is always `serde_json` data.
    if let (Some((_, b'[')), Some((j, b']'))) = (before, after) {
        let rest = m[j + 1..].trim_ascii_start();
        let data_store =
            syn.index_store_is_data && rest.starts_with(b"=") && !rest.starts_with(b"==");
        if !data_store {
            return Some((Shape::StringKey, None));
        }
    }
    // `{"save": …}` / `"save" => …`, but not `c ? "save" : x` nor `case "save":`.
    let key_follows = match after {
        Some((j, b':')) => syn.key_colon && m.get(j + 1) != Some(&b':'),
        Some((j, b'=')) => syn.key_rocket && m.get(j + 1) == Some(&b'>'),
        _ => false,
    };
    if key_follows
        && !matches!(before, Some((_, b'?')))
        && word_before(m, lit.open, syn).as_deref() != Some("case")
    {
        return Some((Shape::StringKey, None));
    }
    None
}

/// Every dynamic-dispatch site naming `name` in one source text, one per
/// line (most specific shape), in line order. Empty for a language without a
/// shape table, or a name that is not an identifier.
pub fn scan_source(language: &str, source: &str, name: &str) -> Vec<ShapeHit> {
    scan_source_with_defs(language, source, name, &[])
}

/// [`scan_source`], told which 1-based lines hold a definition of `name`.
pub fn scan_source_with_defs(
    language: &str,
    source: &str,
    name: &str,
    def_lines: &[usize],
) -> Vec<ShapeHit> {
    let Some(syn) = syntax_for(language) else {
        return Vec::new();
    };
    if name.is_empty() || !source.contains(name) {
        return Vec::new();
    }
    let src = source.as_bytes();
    let Lexed { masked: m, strings } = lex(src, &syn);
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            src.iter()
                .enumerate()
                .filter(|(_, &b)| b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    let line_of_pos = |pos: usize| line_starts.partition_point(|&s| s <= pos);

    let mut hits: std::collections::BTreeMap<usize, (Shape, Option<String>)> =
        std::collections::BTreeMap::new();
    let mut add = |pos: usize, shape: Shape, via: Option<String>| {
        let line = line_of_pos(pos);
        match hits.get(&line) {
            Some((s, _)) if *s <= shape => {}
            _ => {
                hits.insert(line, (shape, via));
            }
        }
    };

    for lit in &strings {
        if let Some((shape, via)) = classify_string(&m, lit, name, &syn) {
            add(lit.open, shape, via);
        }
    }

    let nb = name.as_bytes();
    let mut occurrences = Vec::new();
    let mut from = 0;
    while let Some(s) = find_sub(&m, from, nb) {
        let e = s + nb.len();
        from = s + 1;
        let left_ok = s == 0 || !is_ident_byte(m[s - 1], &syn);
        let right_ok = e >= m.len()
            || !(is_ident_byte(m[e], &syn) || (syn.ruby && matches!(m[e], b'?' | b'!')));
        if left_ok && right_ok {
            occurrences.push((s, e));
        }
    }
    // A local binding of the name (not the definition itself) shadows every
    // bare use in the file; qualified uses (`obj.save`) still count.
    let shadowed = !syn.ruby
        && occurrences
            .iter()
            .any(|&(s, e)| is_binding(&m, s, e, &syn) && !def_lines.contains(&line_of_pos(s)));

    for &(s, e) in &occurrences {
        if syn.ruby {
            // `:save` — a symbol, not `Foo::save` and not `a ?b :save`.
            if s >= 1
                && m[s - 1] == b':'
                && (s < 2 || !(m[s - 2] == b':' || is_ident_byte(m[s - 2], &syn)))
            {
                let callee =
                    enclosing_callee(&m, s - 1, &syn).or_else(|| parenless_callee(&m, s - 1, &syn));
                match callee.as_deref().and_then(classify_callee) {
                    Some(shape) => add(s, shape, callee),
                    None => add(s, Shape::Symbol, None),
                }
            }
            continue;
        }
        if s > 0 && matches!(m[s - 1], b'@' | b'$') {
            continue;
        }
        if shadowed && is_bare(&m, s) {
            continue;
        }
        if is_function_reference(&m, s, e, &syn) {
            add(s, Shape::FunctionReference, None);
        }
    }

    hits.into_iter()
        .map(|(line, (shape, via))| ShapeHit { line, shape, via })
        .collect()
}

/// One reported site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundarySite {
    pub file_path: String,
    pub line: usize,
    pub shape: Shape,
    pub via: Option<String>,
}

/// The disclosure attached to an empty caller result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boundaries {
    /// The bare name that was scanned for.
    pub name: String,
    /// Every site found, ordered by (file, line). Rendering caps it.
    pub sites: Vec<BoundarySite>,
}

impl Boundaries {
    /// The follow-up that shows every textual occurrence, comments and tests
    /// included — the superset this scan narrowed.
    pub fn next_command(&self) -> String {
        let quoted = if self
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.name.clone()
        } else {
            format!("'{}'", self.name.replace('\'', "'\\''"))
        };
        format!("code-graph-mcp grep -w -F {quoted}")
    }

    /// Additive JSON field `boundaries`.
    pub fn to_json(&self) -> serde_json::Value {
        let sites: Vec<serde_json::Value> = self
            .sites
            .iter()
            .take(BOUNDARY_SITE_CAP)
            .map(|s| {
                let mut v = serde_json::json!({
                    "file_path": s.file_path,
                    "line": s.line,
                    "shape": s.shape.as_str(),
                });
                if let Some(via) = &s.via {
                    v["via"] = serde_json::json!(via);
                }
                v
            })
            .collect();
        if self.sites.is_empty() {
            return serde_json::json!({ "total": 0, "sites": sites });
        }
        serde_json::json!({
            "total": self.sites.len(),
            "sites": sites,
            "note": "name used in dynamic-dispatch shapes the static graph does not follow; not edges",
            "next": self.next_command(),
        })
    }

    /// Text block printed after an empty result, each line prefixed by `indent`.
    pub fn render_text<W: std::io::Write>(&self, out: &mut W, indent: &str) -> std::io::Result<()> {
        if self.sites.is_empty() {
            return writeln!(
                out,
                "{indent}(no dynamic-dispatch site names '{}')",
                self.name
            );
        }
        writeln!(
            out,
            "{indent}{} dynamic-dispatch site(s) name '{}' (not graph edges):",
            self.sites.len(),
            self.name
        )?;
        for s in self.sites.iter().take(BOUNDARY_SITE_CAP) {
            match &s.via {
                Some(via) => writeln!(
                    out,
                    "{indent}  {}:{}  {} ({via})",
                    s.file_path,
                    s.line,
                    s.shape.label()
                )?,
                None => writeln!(
                    out,
                    "{indent}  {}:{}  {}",
                    s.file_path,
                    s.line,
                    s.shape.label()
                )?,
            }
        }
        if self.sites.len() > BOUNDARY_SITE_CAP {
            writeln!(
                out,
                "{indent}  … {} more",
                self.sites.len() - BOUNDARY_SITE_CAP
            )?;
        }
        writeln!(out, "{indent}  next: {}", self.next_command())
    }
}

/// Is this a file the scan reads? Production code in a language with a
/// shape table.
fn scannable(path: &str, language: Option<&str>) -> bool {
    language.and_then(syntax_for).is_some()
        && !crate::domain::is_test_path(path)
        && !path.ends_with("_spec.rb")
}

/// Scan every indexed production file for `name`, skipping the given
/// `(file, line)` definition sites. Files are read from disk; one that fails
/// to read (deleted since indexing, not UTF-8, too large) is skipped.
pub fn scan_project(
    conn: &Connection,
    project_root: &Path,
    name: &str,
    exclude: &[(String, usize)],
) -> Result<Boundaries> {
    let files: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare("SELECT path, language FROM files ORDER BY path")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut sites = Vec::new();
    for (path, language) in files {
        if !scannable(&path, language.as_deref()) {
            continue;
        }
        let abs = project_root.join(&path);
        match std::fs::metadata(&abs) {
            Ok(md) if md.len() <= MAX_SCAN_BYTES => {}
            _ => continue,
        }
        let Ok(bytes) = std::fs::read(&abs) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if !text.contains(name) {
            continue;
        }
        let def_lines: Vec<usize> = exclude
            .iter()
            .filter(|(f, _)| *f == path)
            .map(|(_, l)| *l)
            .collect();
        let language = language.as_deref().unwrap_or_default();
        for hit in scan_source_with_defs(language, text, name, &def_lines) {
            if def_lines.contains(&hit.line) {
                continue;
            }
            sites.push(BoundarySite {
                file_path: path.clone(),
                line: hit.line,
                shape: hit.shape,
                via: hit.via,
            });
        }
    }
    Ok(Boundaries {
        name: name.to_string(),
        sites,
    })
}

/// Last segment of a possibly qualified symbol (`Store::save`, `Store.save`).
pub fn bare_name(symbol: &str) -> &str {
    let tail = symbol.rsplit("::").next().unwrap_or(symbol);
    tail.rsplit('.').next().unwrap_or(tail)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Boundaries for an empty caller result on `symbol`, or `None` when the
/// disclosure does not apply: the name is not an identifier, or no definition
/// of it is a function or method (a type or constant is not dispatched to).
pub fn for_empty_result(
    conn: &Connection,
    project_root: &Path,
    symbol: &str,
) -> Result<Option<Boundaries>> {
    let name = bare_name(symbol);
    if !is_identifier(name) {
        return Ok(None);
    }
    let defs = crate::storage::queries::get_nodes_with_files_by_name(conn, name)?;
    if !defs
        .iter()
        .any(|d| crate::domain::is_function_node_type(&d.node.node_type))
    {
        return Ok(None);
    }
    let exclude: Vec<(String, usize)> = defs
        .iter()
        .map(|d| (d.file_path.clone(), d.node.start_line.max(0) as usize))
        .collect();
    scan_project(conn, project_root, name, &exclude).map(Some)
}

#[cfg(test)]
mod tests;
