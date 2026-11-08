//! Parser for the Beancount subset `BranchBooks` uses: option, include, plugin (recorded, not run),
//! open, close, balance, transactions with postings and metadata, custom. Other dated directives that never
//! move money (commodity, price, event, note, document, query) are accepted and kept as `Other`.

use std::sync::Arc;

use crate::date::Date;
use crate::money::{parse_decimal, scale_of};
use crate::types::{
    Amount, Balance, Body, Close, Custom, CustomValue, Diagnostic, Entry, Meta, MetaValue, Open,
    Other, Posting, Transaction,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionLine {
    pub key: String,
    pub value: String,
    pub file: Arc<str>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncludeLine {
    pub path: String,
    pub file: Arc<str>,
    pub line: u32,
    /// Index into `entries` at the point of the include.
    pub at: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseResult {
    pub file: Arc<str>,
    pub entries: Vec<Entry>,
    pub options: Vec<OptionLine>,
    pub includes: Vec<IncludeLine>,
    pub plugins: Vec<OptionLine>,
    pub errors: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Str,
    Date,
    Num,
    Acct,
    Key,
    Cur,
    Tag,
    Link,
    Kw,
    Sym,
}

#[derive(Clone, Copy, Debug)]
struct Tok<'a> {
    k: Kind,
    v: &'a str,
}

struct SyntaxError(String);

type Result<T> = std::result::Result<T, SyntaxError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(SyntaxError(message.into()))
}

const ROOTS: [&str; 5] = ["Assets", "Liabilities", "Equity", "Income", "Expenses"];
const IGNORED: [&str; 6] = ["commodity", "price", "event", "note", "document", "query"];
const UNSUPPORTED_DATED: [&str; 1] = ["pad"];
const UNSUPPORTED_UNDATED: [&str; 4] = ["pushtag", "poptag", "pushmeta", "popmeta"];
const SYMBOLS: [&str; 10] = ["@@", "@", "{{", "{", "}}", "}", ",", "*", "!", "~"];

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

fn run(b: &[u8], mut i: usize, ok: impl Fn(u8) -> bool) -> usize {
    while i < b.len() && ok(b[i]) {
        i += 1;
    }
    i
}

/// The token at `i`, trying each alternative in order. The comment
/// alternative comes back as `None` with `Some(i)`: everything from there is ignored.
fn match_at(text: &str, i: usize) -> Option<(Option<Kind>, usize)> {
    let b = text.as_bytes();
    let n = b.len();
    let at = |j: usize| b.get(j).copied();
    let digit = |j: usize| at(j).is_some_and(|c| c.is_ascii_digit());

    // 1. string: "(?:[^"\\]|\\.)*".
    if b[i] == b'"' {
        let mut chars = text[i + 1..].char_indices();
        while let Some((off, c)) = chars.next() {
            match c {
                '"' => return Some((Some(Kind::Str), i + 1 + off + 1)),
                '\\' => match chars.next() {
                    Some((_, '\n' | '\r')) | None => return None,
                    Some(_) => {}
                },
                _ => {}
            }
        }
        return None;
    }
    // 2. comment
    if b[i] == b';' {
        return Some((None, n));
    }
    // 3. date, not followed by a digit or '-'
    if (0..4).all(|k| digit(i + k))
        && at(i + 4) == Some(b'-')
        && digit(i + 5)
        && digit(i + 6)
        && at(i + 7) == Some(b'-')
        && digit(i + 8)
        && digit(i + 9)
        && !at(i + 10).is_some_and(|c| c.is_ascii_digit() || c == b'-')
    {
        return Some((Some(Kind::Date), i + 10));
    }
    // 4. number: [-+]?(?:\d[\d,]*(?:\.\d*)?|\.\d+)
    {
        let s = if matches!(b[i], b'-' | b'+') {
            i + 1
        } else {
            i
        };
        if digit(s) {
            let mut j = run(b, s + 1, |c| c.is_ascii_digit() || c == b',');
            if at(j) == Some(b'.') {
                j = run(b, j + 1, |c| c.is_ascii_digit());
            }
            return Some((Some(Kind::Num), j));
        }
        if at(s) == Some(b'.') && digit(s + 1) {
            return Some((Some(Kind::Num), run(b, s + 1, |c| c.is_ascii_digit())));
        }
    }
    let c = b[i];
    if c.is_ascii_uppercase() {
        // 5. account: [A-Z][A-Za-z0-9-]*(?::[A-Z0-9][A-Za-z0-9-]*)+
        let mut j = run(b, i + 1, is_word);
        let mut parts = 0;
        while at(j) == Some(b':')
            && at(j + 1).is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            j = run(b, j + 2, is_word);
            parts += 1;
        }
        if parts > 0 {
            return Some((Some(Kind::Acct), j));
        }
        // 7. currency (also TRUE, FALSE, NULL): [A-Z][A-Z0-9'._-]*
        let j = run(b, i + 1, |c| {
            c.is_ascii_uppercase() || c.is_ascii_digit() || b"'._-".contains(&c)
        });
        return Some((Some(Kind::Cur), j));
    }
    if c.is_ascii_lowercase() {
        // 6. metadata key: [a-z][a-zA-Z0-9_-]*: followed by whitespace or the end
        let j = run(b, i + 1, |c| {
            c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
        });
        if at(j) == Some(b':') && text[j + 1..].chars().next().is_none_or(char::is_whitespace) {
            return Some((Some(Kind::Key), j + 1));
        }
        // 10. keyword: [a-z][a-z_]*
        return Some((
            Some(Kind::Kw),
            run(b, i + 1, |c| c.is_ascii_lowercase() || c == b'_'),
        ));
    }
    // 8. tag, 9. link
    if c == b'#' || c == b'^' {
        let j = run(b, i + 1, |c| {
            c.is_ascii_alphanumeric() || b"_/.-".contains(&c)
        });
        if j > i + 1 {
            return Some((Some(if c == b'#' { Kind::Tag } else { Kind::Link }), j));
        }
        return None;
    }
    // 11. symbol
    SYMBOLS
        .iter()
        .find(|s| text[i..].starts_with(*s))
        .map(|s| (Some(Kind::Sym), i + s.len()))
}

fn tokenize(text: &str, from: usize) -> Result<Vec<Tok<'_>>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = from;
    loop {
        i = run(b, i, |c| c == b' ' || c == b'\t');
        if i >= b.len() {
            return Ok(out);
        }
        match match_at(text, i) {
            Some((None, _)) => return Ok(out),
            Some((Some(k), end)) => {
                out.push(Tok {
                    k,
                    v: &text[i..end],
                });
                i = end;
            }
            // Beancount refuses a BOM too (docs/DESIGN.md), so say what it is and how to fix it.
            None if text[i..].starts_with('\u{feff}') => {
                return fail(format!(
                    "byte order mark at column {}: save the file as UTF-8 without BOM",
                    text[..i].chars().count() + 1
                ));
            }
            None => {
                return fail(format!(
                    "unexpected input at column {}: {:?}",
                    text[..i].chars().count() + 1,
                    text[i..].chars().take(20).collect::<String>()
                ));
            }
        }
    }
}

/// A line's tokens, without spacing or a trailing comment, or `None` if it doesn't lex.
pub(crate) fn line_tokens(line: &str) -> Option<Vec<&str>> {
    tokenize(line, 0)
        .ok()
        .map(|t| t.into_iter().map(|t| t.v).collect())
}

fn unquote(s: &str) -> String {
    let inner = &s[1..s.len() - 1];
    if !inner.contains('\\') {
        return inner.to_owned();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(c) => out.push(c),
            None => out.push('\\'),
        }
    }
    out
}

fn parse_number(text: &str) -> Result<(crate::money::Decimal, u32)> {
    let clean = text.replace(',', "");
    let Some(number) = parse_decimal(&clean) else {
        return fail(format!("number out of range: {clean}"));
    };
    Ok((number, scale_of(clean.trim_start_matches(['-', '+']))))
}

fn check_account(name: &str) -> Result<String> {
    let root = &name[..name.find(':').unwrap_or(name.len())];
    if !ROOTS.contains(&root) {
        return fail(format!(
            "invalid account name {name}: root must be one of {}",
            ROOTS.join(", ")
        ));
    }
    Ok(name.to_owned())
}

/// Cursor over one line's tokens.
struct Cursor<'a> {
    i: usize,
    toks: Vec<Tok<'a>>,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<Tok<'a>> {
        self.toks.get(self.i).copied()
    }

    fn done(&self) -> bool {
        self.i >= self.toks.len()
    }

    fn take(&mut self, k: Kind, what: &str) -> Result<&'a str> {
        match self.peek() {
            Some(t) if t.k == k => {
                self.i += 1;
                Ok(t.v)
            }
            Some(t) => fail(format!("expected {what}, found {:?}", t.v)),
            None => fail(format!("expected {what} at end of line")),
        }
    }

    fn take_if(&mut self, k: Kind) -> Option<&'a str> {
        let t = self.peek().filter(|t| t.k == k)?;
        self.i += 1;
        Some(t.v)
    }

    fn take_sym(&mut self, v: &str) -> Option<&'a str> {
        let t = self.peek().filter(|t| t.k == Kind::Sym && t.v == v)?;
        self.i += 1;
        Some(t.v)
    }

    fn end(&self, what: &str) -> Result<()> {
        match self.peek() {
            Some(t) => fail(format!("unexpected {:?} after {what}", t.v)),
            None => Ok(()),
        }
    }
}

fn read_amount_after_number(cur: &mut Cursor, num_text: &str) -> Result<Amount> {
    let (number, scale) = parse_number(num_text)?;
    let currency = cur.take(Kind::Cur, "currency after number")?.to_owned();
    Ok(Amount {
        number,
        currency,
        scale,
    })
}

fn read_meta_value(cur: &mut Cursor) -> Result<MetaValue> {
    let Some(t) = cur.peek() else {
        return Ok(MetaValue::Null);
    };
    cur.i += 1;
    Ok(match t.k {
        Kind::Str => MetaValue::String(unquote(t.v)),
        Kind::Acct => MetaValue::Account(t.v.to_owned()),
        Kind::Date => MetaValue::Date(parse_date(t.v)?),
        Kind::Tag => MetaValue::Tag(t.v[1..].to_owned()),
        Kind::Num if cur.peek().is_some_and(|n| n.k == Kind::Cur) => {
            MetaValue::Amount(read_amount_after_number(cur, t.v)?)
        }
        Kind::Num => MetaValue::Number(parse_number(t.v)?.0),
        Kind::Cur => match t.v {
            "TRUE" => MetaValue::Bool(true),
            "FALSE" => MetaValue::Bool(false),
            "NULL" => MetaValue::Null,
            v => MetaValue::Currency(v.to_owned()),
        },
        _ => return fail(format!("invalid metadata value {:?}", t.v)),
    })
}

fn read_custom_value(cur: &mut Cursor) -> Result<CustomValue> {
    let t = cur.peek().expect("caller checks for more tokens");
    cur.i += 1;
    Ok(match t.k {
        Kind::Str => CustomValue::String(unquote(t.v)),
        Kind::Acct => CustomValue::Account(check_account(t.v)?),
        Kind::Date => CustomValue::Date(parse_date(t.v)?),
        Kind::Num if cur.peek().is_some_and(|n| n.k == Kind::Cur) => {
            CustomValue::Amount(read_amount_after_number(cur, t.v)?)
        }
        Kind::Num => CustomValue::Number(parse_number(t.v)?.0),
        Kind::Cur if t.v == "TRUE" || t.v == "FALSE" => CustomValue::Bool(t.v == "TRUE"),
        _ => return fail(format!("invalid custom value {:?}", t.v)),
    })
}

/// State of the directive currently accepting indented lines.
struct Open_ {
    entry: Option<Entry>,
    /// The header or a posting failed: drop the directive and skip its indented lines.
    bad: bool,
    last_posting: Option<usize>,
    posting_indent: usize,
}

impl Open_ {
    fn finish(&mut self, entries: &mut Vec<Entry>) {
        if let Some(e) = self.entry.take()
            && !self.bad
        {
            entries.push(e);
        }
        self.bad = false;
        self.last_posting = None;
    }
}

pub fn parse(text: &str, file: &str) -> ParseResult {
    let file: Arc<str> = Arc::from(file);
    let mut res = ParseResult {
        file: file.clone(),
        ..Default::default()
    };
    let mut cur = Open_ {
        entry: None,
        bad: false,
        last_posting: None,
        posting_indent: 0,
    };

    for (idx, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let line_no = idx as u32 + 1;
        let Some(&first) = raw.as_bytes().first() else {
            continue;
        };

        if first == b' ' || first == b'\t' {
            // Indented: metadata or posting for the open directive.
            let indent = run(raw.as_bytes(), 0, |c| c == b' ' || c == b'\t');
            if indent == raw.len() || raw.as_bytes()[indent] == b';' {
                continue;
            }
            let Some(entry) = cur.entry.as_mut() else {
                if !cur.bad {
                    res.errors.push(Diagnostic::new(
                        &file,
                        line_no,
                        "syntax",
                        "indented line outside a directive",
                    ));
                }
                continue;
            };
            if cur.bad {
                continue;
            }
            match indented_line(
                entry,
                raw,
                indent,
                line_no,
                cur.last_posting,
                cur.posting_indent,
            ) {
                Ok(Some(posting)) => {
                    cur.last_posting = Some(posting);
                    cur.posting_indent = indent;
                }
                Ok(None) => {}
                Err(SyntaxError(message)) => {
                    res.errors
                        .push(Diagnostic::new(&file, line_no, "syntax", message));
                    cur.bad = true;
                }
            }
            continue;
        }

        // A top-level line ends the previous directive.
        cur.finish(&mut res.entries);
        if matches!(first, b';' | b'*' | b'#') || raw.trim().is_empty() {
            continue;
        }
        match top_level_line(&mut res, &file, raw, line_no) {
            Ok(entry) => cur.entry = entry,
            Err(SyntaxError(message)) => {
                res.errors
                    .push(Diagnostic::new(&file, line_no, "syntax", message));
                cur.entry = None;
                cur.bad = true;
            }
        }
    }
    cur.finish(&mut res.entries);
    res
}

/// Metadata or a posting under `entry`. Returns the new posting's index, if it was a posting.
fn indented_line(
    entry: &mut Entry,
    raw: &str,
    indent: usize,
    line_no: u32,
    last_posting: Option<usize>,
    posting_indent: usize,
) -> Result<Option<usize>> {
    let mut c = Cursor {
        i: 0,
        toks: tokenize(raw, indent)?,
    };
    if let Some(key_tok) = c.take_if(Kind::Key) {
        let key = key_tok[..key_tok.len() - 1].to_owned();
        let value = read_meta_value(&mut c)?;
        c.end("metadata value")?;
        let posting_meta = match (&mut entry.body, last_posting) {
            (Body::Txn(t), Some(p)) if indent > posting_indent => Some(&mut t.postings[p].meta),
            _ => None,
        };
        posting_meta.unwrap_or(&mut entry.meta).set(key, value);
        entry.end_line = line_no;
        return Ok(None);
    }
    let Body::Txn(txn) = &mut entry.body else {
        return fail("postings are only allowed in transactions");
    };
    let flag = c
        .take_sym("*")
        .or_else(|| c.take_sym("!"))
        .map(str::to_owned);
    let account = check_account(c.take(Kind::Acct, "account")?)?;
    let units = match c.take_if(Kind::Num) {
        Some(num) => Some(read_amount_after_number(&mut c, num)?),
        None => None,
    };
    if let Some(next) = c.peek()
        && next.k == Kind::Sym
        && (next.v.starts_with('{') || next.v.starts_with('@'))
    {
        return fail("costs and prices are not supported by BranchBooks yet");
    }
    c.end("posting")?;
    txn.postings.push(Posting {
        account,
        units,
        flag,
        meta: Meta::default(),
        line: line_no,
    });
    entry.end_line = line_no;
    Ok(Some(txn.postings.len() - 1))
}

/// An undated directive (recorded in `res`) or the header of a dated one (returned).
fn top_level_line(
    res: &mut ParseResult,
    file: &Arc<str>,
    raw: &str,
    line_no: u32,
) -> Result<Option<Entry>> {
    let mut c = Cursor {
        i: 0,
        toks: tokenize(raw, 0)?,
    };
    let Some(head) = c.peek() else {
        return Ok(None);
    };
    if head.k == Kind::Kw {
        c.i += 1;
        match head.v {
            "option" => {
                let key = unquote(c.take(Kind::Str, "option name")?);
                let value = unquote(c.take(Kind::Str, "option value")?);
                c.end("option")?;
                res.options.push(OptionLine {
                    key,
                    value,
                    file: file.clone(),
                    line: line_no,
                });
            }
            "include" => {
                let path = unquote(c.take(Kind::Str, "include path")?);
                c.end("include")?;
                res.includes.push(IncludeLine {
                    path,
                    file: file.clone(),
                    line: line_no,
                    at: res.entries.len(),
                });
            }
            "plugin" => {
                let key = unquote(c.take(Kind::Str, "plugin name")?);
                let value = c.take_if(Kind::Str).map(unquote).unwrap_or_default();
                c.end("plugin")?;
                res.plugins.push(OptionLine {
                    key,
                    value,
                    file: file.clone(),
                    line: line_no,
                });
            }
            v if UNSUPPORTED_UNDATED.contains(&v) => {
                return fail(format!("{v} is not supported by BranchBooks yet"));
            }
            v => return fail(format!("unknown directive {v}")),
        }
        return Ok(None);
    }

    let date = parse_date(c.take(Kind::Date, "date or directive")?)?;
    let Some(kw) = c.peek() else {
        return fail("expected directive after date");
    };
    c.i += 1;

    let body = match (kw.k, kw.v) {
        (Kind::Kw, "open") => {
            let account = check_account(c.take(Kind::Acct, "account")?)?;
            let mut currencies = Vec::new();
            if let Some(first) = c.take_if(Kind::Cur) {
                currencies.push(first.to_owned());
                while c.take_sym(",").is_some() {
                    currencies.push(c.take(Kind::Cur, "currency")?.to_owned());
                }
            }
            let booking = c.take_if(Kind::Str).map(unquote);
            c.end("open")?;
            Body::Open(Open {
                account,
                currencies,
                booking,
            })
        }
        (Kind::Kw, "close") => {
            let account = check_account(c.take(Kind::Acct, "account")?)?;
            c.end("close")?;
            Body::Close(Close { account })
        }
        (Kind::Kw, "balance") => {
            let account = check_account(c.take(Kind::Acct, "account")?)?;
            let (number, scale) = parse_number(c.take(Kind::Num, "amount")?)?;
            let tolerance = match c.take_sym("~") {
                Some(_) => Some(parse_number(c.take(Kind::Num, "tolerance")?)?.0),
                None => None,
            };
            let currency = c.take(Kind::Cur, "currency after number")?.to_owned();
            c.end("balance")?;
            Body::Balance(Balance {
                account,
                amount: Amount {
                    number,
                    currency,
                    scale,
                },
                tolerance,
            })
        }
        (Kind::Kw, "custom") => {
            let name = unquote(c.take(Kind::Str, "custom type")?);
            let mut values = Vec::new();
            while !c.done() {
                values.push(read_custom_value(&mut c)?);
            }
            Body::Custom(Custom { name, values })
        }
        (Kind::Sym, "*" | "!") | (Kind::Kw, "txn") => {
            let mut strings = Vec::new();
            while let Some(s) = c.take_if(Kind::Str) {
                strings.push(unquote(s));
            }
            if strings.len() > 2 {
                return fail("too many strings on transaction line");
            }
            let (mut tags, mut links) = (Vec::new(), Vec::new());
            while let Some(t) = c.peek() {
                match t.k {
                    Kind::Tag => tags.push(t.v[1..].to_owned()),
                    Kind::Link => links.push(t.v[1..].to_owned()),
                    _ => break,
                }
                c.i += 1;
            }
            c.end("transaction header")?;
            let mut strings = strings.into_iter();
            let (payee, narration) = match (strings.next(), strings.next()) {
                (Some(p), Some(n)) => (Some(p), n),
                (Some(n), None) => (None, n),
                _ => (None, String::new()),
            };
            let flag = if kw.v == "txn" { "*" } else { kw.v }.to_owned();
            Body::Txn(Transaction {
                flag,
                payee,
                narration,
                tags,
                links,
                postings: Vec::new(),
            })
        }
        (Kind::Kw, v) if IGNORED.contains(&v) => Body::Other(Other {
            keyword: v.to_owned(),
        }),
        (Kind::Kw, v) if UNSUPPORTED_DATED.contains(&v) => {
            return fail(format!("{v} is not supported by BranchBooks yet"));
        }
        (_, v) => return fail(format!("unknown directive {v:?}")),
    };
    Ok(Some(Entry {
        date,
        file: file.clone(),
        line: line_no,
        end_line: line_no,
        seq: 0,
        meta: Meta::default(),
        body,
    }))
}

fn parse_date(date: &str) -> Result<Date> {
    date.parse()
        .map_err(|_| SyntaxError(format!("invalid date {date}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, &str)> {
        tokenize(text, 0)
            .ok()
            .unwrap()
            .into_iter()
            .map(|t| (t.k, t.v))
            .collect()
    }

    #[test]
    fn tokens_are_matched_in_a_fixed_order() {
        use Kind::*;
        assert_eq!(
            kinds(
                r#"2026-10-01 * "a \"b\"" Assets:Cash-1 -1,234.50 USD key: TRUE #t ^l {{ @@ ; c"#
            ),
            vec![
                (Date, "2026-10-01"),
                (Sym, "*"),
                (Str, r#""a \"b\"""#),
                (Acct, "Assets:Cash-1"),
                (Num, "-1,234.50"),
                (Cur, "USD"),
                (Key, "key:"),
                (Cur, "TRUE"),
                (Tag, "#t"),
                (Link, "^l"),
                (Sym, "{{"),
                (Sym, "@@"),
            ]
        );
        // A date followed by a digit or '-' is a number, then more numbers.
        assert_eq!(
            kinds("2026-10-011"),
            vec![(Num, "2026"), (Num, "-10"), (Num, "-011")]
        );
        // A key needs whitespace or the end after its colon; otherwise it is a keyword.
        assert_eq!(kinds("foo: foo:"), vec![(Key, "foo:"), (Key, "foo:")]);
        // Currencies are upper case only, so a bare root splits (it is an error later either way).
        assert_eq!(kinds("Assets"), vec![(Cur, "A"), (Kw, "ssets")]);
        assert_eq!(kinds(".5 5."), vec![(Num, ".5"), (Num, "5.")]);
    }

    #[test]
    fn unexpected_input_reports_a_character_column() {
        let Err(SyntaxError(m)) = tokenize("\"é\" %rest", 0) else {
            panic!()
        };
        assert_eq!(m, r#"unexpected input at column 5: "%rest""#);
        let Err(SyntaxError(m)) = tokenize("\"unterminated", 0) else {
            panic!()
        };
        assert_eq!(m, r#"unexpected input at column 1: "\"unterminated""#);
        let Err(SyntaxError(m)) = tokenize("\"😀\" %rest", 0) else {
            panic!()
        };
        assert_eq!(m, r#"unexpected input at column 5: "%rest""#);
    }

    #[test]
    fn a_byte_order_mark_is_named_with_the_fix() {
        let Err(SyntaxError(m)) = tokenize("\u{feff}2026-01-01 open Assets:Cash", 0) else {
            panic!()
        };
        assert_eq!(
            m,
            "byte order mark at column 1: save the file as UTF-8 without BOM"
        );
    }
}
