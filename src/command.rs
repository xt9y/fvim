use regex::{Captures, Regex, RegexBuilder};

// Translate the common Vim 'magic' dialect; unsupported assertions and pattern
// backreferences are rejected rather than silently changing their meaning.
pub fn compile(pattern: &str, case: Option<bool>) -> Result<Regex, String> {
    let mut result = String::new();
    let mut chars = pattern.chars().peekable();
    let mut magic = 'm';
    let mut in_class = false;
    let mut insensitive = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let next = chars.next().ok_or("Invalid pattern: trailing backslash")?;
            match next {
                'v' | 'V' | 'm' | 'M' => magic = next,
                'c' => insensitive = true,
                'C' => insensitive = false,
                '<' | '>' => result.push_str(r"\b"),
                '0'..='9' | '@' | 'z' | '%' | '_' => {
                    return Err(format!("Invalid pattern: unsupported Vim escape \\{next}"))
                }
                '{' if magic != 'v' && magic != 'V' => {
                    let mut repetition = String::new();
                    let mut closed = false;
                    for ch in chars.by_ref() {
                        if ch == '}' {
                            closed = true;
                            break;
                        }
                        repetition.push(ch);
                    }
                    if !closed || repetition.chars().any(|c| !c.is_ascii_digit() && c != ',') {
                        return Err(
                            "Invalid pattern: unsupported or unclosed Vim repetition".into()
                        );
                    }
                    result.push('{');
                    if repetition.starts_with(',') {
                        result.push('0');
                    }
                    result.push_str(&repetition);
                    result.push('}');
                }
                '(' | ')' | '+' | '?' | '|' | '}' if magic != 'v' && magic != 'V' => {
                    result.push(next)
                }
                '=' if magic != 'v' && magic != 'V' => result.push('?'),
                '.' | '*' | '[' if magic == 'M' => {
                    result.push(next);
                    if next == '[' {
                        in_class = true;
                    }
                }
                'd' | 'D' | 's' | 'S' | 'w' | 'W' | 't' | 'n' | 'r' | 'f' => {
                    result.push('\\');
                    result.push(next);
                }
                'b' => result.push_str(r"\x08"),
                other => result.push_str(&regex::escape(&other.to_string())),
            }
            continue;
        }
        if in_class {
            result.push(ch);
            if ch == ']' {
                in_class = false;
            }
            continue;
        }
        let special = match magic {
            'v' => ".[]^$*+?(){}|".contains(ch),
            'm' => ".[]^$*".contains(ch),
            'M' => "^$".contains(ch),
            _ => false,
        };
        if special {
            result.push(ch);
            if ch == '[' {
                in_class = true;
            }
        } else {
            result.push_str(&regex::escape(&ch.to_string()));
        }
    }
    RegexBuilder::new(&result)
        .multi_line(true)
        .case_insensitive(case.unwrap_or(insensitive))
        .size_limit(2 * 1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|e| format!("Invalid pattern: {e}"))
}

pub struct Search {
    pub pattern: String,
    pub forward: bool,
    pub regex: Regex,
}

impl Search {
    pub fn new(pattern: String, forward: bool) -> Result<Self, String> {
        let regex = compile(&pattern, None)?;
        Ok(Self {
            pattern,
            forward,
            regex,
        })
    }

    pub fn destination(
        &self,
        body: &str,
        pos: usize,
        forward: bool,
        count: usize,
    ) -> Option<usize> {
        let count = count.max(1);
        let (mut total, mut byte, mut chars) = (0, 0, 0);
        let (mut first_after, mut last_before) = (None, None);
        for m in self.regex.find_iter(body) {
            // Match byte offsets are sorted: decode each intervening span once.
            chars += body[byte..m.start()].chars().count();
            byte = m.start();
            if chars > pos && forward {
                let first = *first_after.get_or_insert(total);
                if total - first == count - 1 {
                    return Some(chars);
                }
            }
            if chars < pos {
                last_before = Some(total);
            }
            total += 1;
        }
        if total == 0 {
            return None;
        }
        let first = if forward {
            first_after.unwrap_or(0)
        } else {
            last_before.unwrap_or(total - 1)
        };
        let n = (count - 1) % total;
        let index = if forward {
            (first + n) % total
        } else {
            (first + total - n) % total
        };
        // A wrapping/backward search needs at most one second pass, no match list.
        self.regex
            .find_iter(body)
            .nth(index)
            .map(|m| body[..m.start()].chars().count())
    }
}

pub fn range(
    text: &str,
    current: usize,
    length: usize,
    visual: Option<(usize, usize)>,
) -> Result<(usize, usize, &str, bool), String> {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('%') {
        return Ok((0, length, rest.trim_start(), true));
    }
    fn address(
        text: &str,
        current: usize,
        length: usize,
        visual: Option<(usize, usize)>,
    ) -> Result<Option<(usize, &str)>, String> {
        let mut rest = text;
        let mut n;
        if let Some(t) = rest.strip_prefix('.') {
            n = current as isize;
            rest = t;
        } else if let Some(t) = rest.strip_prefix('$') {
            n = length as isize - 1;
            rest = t;
        } else if let Some(t) = rest.strip_prefix("'<") {
            n = visual.ok_or("Invalid range: no visual selection")?.0 as isize;
            rest = t;
        } else if let Some(t) = rest.strip_prefix("'>") {
            n = visual.ok_or("Invalid range: no visual selection")?.1 as isize;
            rest = t;
        } else if rest.starts_with(|c: char| c.is_ascii_digit()) {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            n = rest[..end]
                .parse::<isize>()
                .map_err(|_| "Invalid range: address too large")?
                - 1;
            rest = &rest[end..];
        } else if rest.starts_with(['+', '-']) {
            n = current as isize;
        } else {
            return Ok(None);
        }
        while rest.starts_with(['+', '-']) {
            let sign = if rest.starts_with('+') { 1isize } else { -1 };
            rest = &rest[1..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let delta = if end == 0 {
                1
            } else {
                rest[..end]
                    .parse::<isize>()
                    .map_err(|_| "Invalid range: offset too large")?
            };
            n = n
                .checked_add(sign * delta)
                .ok_or("Invalid range: overflow")?;
            rest = &rest[end..];
        }
        if n < 0 || n >= length as isize {
            return Err("Invalid range: line outside buffer".into());
        }
        Ok(Some((n as usize, rest)))
    }
    let Some((start, mut rest)) = address(text, current, length, visual)? else {
        return Ok((current, current + 1, text, false));
    };
    let mut end = start;
    if rest.starts_with([',', ';']) {
        let curr = if rest.starts_with(';') {
            start
        } else {
            current
        };
        rest = &rest[1..];
        let value = address(rest, curr, length, visual)?.ok_or("Invalid range: missing end")?;
        end = value.0;
        rest = value.1;
    }
    if end < start {
        return Err("Invalid range: reversed addresses".into());
    }
    Ok((start, end + 1, rest.trim_start(), true))
}

pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

pub struct Substitute {
    pub pattern: String,
    pub regex: Regex,
    replacement: String,
    pub confirm: bool,
    pub count_only: bool,
    pub suppress_error: bool,
    global: bool,
}

impl Substitute {
    pub fn parse(text: &str, previous: Option<&str>) -> Result<Self, String> {
        let text = text
            .strip_prefix("substitute")
            .or_else(|| text.strip_prefix('s'))
            .ok_or("Invalid substitute")?;
        let delimiter = text
            .chars()
            .next()
            .ok_or("Usage: :s/pattern/replacement/flags")?;
        if delimiter.is_alphanumeric() || delimiter == '\\' || delimiter.is_whitespace() {
            return Err("Invalid substitute delimiter".into());
        }
        fn part(text: &str, delimiter: char) -> Result<(String, &str), String> {
            let mut out = String::new();
            let mut chars = text.char_indices();
            while let Some((i, ch)) = chars.next() {
                if ch == delimiter {
                    return Ok((out, &text[i + ch.len_utf8()..]));
                }
                if ch == '\\' {
                    let (_, next) = chars.next().ok_or("Trailing backslash in substitute")?;
                    if next == delimiter {
                        out.push(next);
                    } else {
                        out.push('\\');
                        out.push(next);
                    }
                } else {
                    out.push(ch);
                }
            }
            Err("Missing substitute delimiter".into())
        }
        let (mut pattern, rest) = part(&text[delimiter.len_utf8()..], delimiter)?;
        // Vim allows the closing replacement delimiter to be omitted.
        let (replacement, flags) = match part(rest, delimiter) {
            Ok(parts) => parts,
            Err(_) if !rest.ends_with('\\') => (rest.to_owned(), ""),
            Err(e) => return Err(e),
        };
        if pattern.is_empty() {
            pattern = previous.ok_or("No previous search pattern")?.to_owned();
        }
        let mut global = false;
        let mut confirm = false;
        let mut count_only = false;
        let mut suppress_error = false;
        let mut case = None;
        for flag in flags.trim().chars() {
            match flag {
                'g' => global = true,
                'c' => confirm = true,
                'i' => case = Some(true),
                'I' => case = Some(false),
                'n' => count_only = true,
                'e' => suppress_error = true,
                _ => return Err(format!("Unsupported substitute flag: {flag}")),
            }
        }
        let regex = compile(&pattern, case)?;
        Ok(Self {
            pattern,
            regex,
            replacement,
            confirm,
            count_only,
            suppress_error,
            global,
        })
    }

    pub fn edits(&self, lines: &[String], start: usize, end: usize) -> Result<Vec<Edit>, String> {
        let mut edits = Vec::new();
        let mut offset = lines
            .iter()
            .take(start)
            .map(|l| l.chars().count() + 1)
            .sum::<usize>();
        for line in &lines[start..end] {
            for caps in self.regex.captures_iter(line) {
                let m = caps.get(0).unwrap();
                edits.push(Edit {
                    start: offset + line[..m.start()].chars().count(),
                    end: offset + line[..m.end()].chars().count(),
                    replacement: expand(&self.replacement, &caps)?,
                });
                if !self.global {
                    break;
                }
            }
            offset += line.chars().count() + 1;
        }
        Ok(edits)
    }
}

fn expand(replacement: &str, caps: &Captures<'_>) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = replacement.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '&' => out.push_str(caps.get(0).unwrap().as_str()),
            '\\' => match chars.next().ok_or("Trailing replacement backslash")? {
                ch @ '0'..='9' => {
                    if let Some(m) = caps.get(ch.to_digit(10).unwrap() as usize) {
                        out.push_str(m.as_str());
                    }
                }
                'r' => out.push('\n'),
                'n' => {
                    return Err(
                        "Replacement \\n (NUL) is unsupported; use \\r for a newline".into(),
                    )
                }
                't' => out.push('\t'),
                '&' => out.push('&'),
                '\\' => out.push('\\'),
                ch => return Err(format!("Unsupported replacement escape: \\{ch}")),
            },
            _ => out.push(ch),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counted_searches_wrap_and_preserve_unicode_and_zero_width_offsets() {
        let s = Search::new("x".into(), true).unwrap();
        let text = "éx\nx🙂x";
        for (pos, forward, count, expected) in [
            (1, true, 1, 3),
            (5, true, 1, 1),
            (3, true, 4, 5),
            (3, false, 1, 1),
            (0, false, 2, 3),
            (5, false, 5, 1),
        ] {
            assert_eq!(s.destination(text, pos, forward, count), Some(expected));
        }
        let s = Search::new("^".into(), true).unwrap();
        assert_eq!(s.destination(text, 0, true, 1), Some(3));
        assert_eq!(s.destination(text, 0, false, 3), Some(3));
        assert_eq!(s.destination("", 0, true, 10), Some(0));
        assert_eq!(
            Search::new("missing".into(), true)
                .unwrap()
                .destination(text, 0, true, 1),
            None
        );
    }

    #[test]
    fn vim_brace_quantifiers_include_the_closing_brace() {
        assert!(compile(r"^a\{2}$", None).unwrap().is_match("aa"));
        assert!(compile(r"^a\{2,3}$", None).unwrap().is_match("aaa"));
        assert!(!compile(r"^a\{2,3}$", None).unwrap().is_match("aaaa"));
    }

    #[test]
    fn vim_magic_literal_and_very_magic_patterns() {
        assert!(compile(r"\(one\|two\)\+", None).unwrap().is_match("onetwo"));
        assert!(compile("a+b", None).unwrap().is_match("a+b"));
        assert!(compile(r"\v(a|b)+", None).unwrap().is_match("abab"));
        assert!(compile(r"\V[one].*", None).unwrap().is_match("[one].*"));
        assert!(compile(r"\<one\>\c", None).unwrap().is_match("ONE"));
        assert!(!compile(r"\<one\>", None).unwrap().is_match("stone"));
        assert!(compile(r"\(a\)\1", None).is_err());
        assert!(compile(r"a\zs", None).is_err());
    }

    #[test]
    fn address_ranges_and_offsets() {
        assert_eq!(
            range("%s/x/y/", 1, 3, None).unwrap(),
            (0, 3, "s/x/y/", true)
        );
        assert_eq!(range(".-1,$d", 1, 3, None).unwrap(), (0, 3, "d", true));
        assert_eq!(range("2;+1y", 0, 4, None).unwrap(), (1, 3, "y", true));
        assert!(range("3,1d", 0, 3, None).is_err());
        assert!(range("0s/a/b/", 0, 3, None).is_err());
    }

    #[test]
    fn substitution_delimiters_empty_replacement_and_zero_width() {
        let s = Substitute::parse(r"s#a/b#x\#y#g", None).unwrap();
        let edits = s.edits(&["a/b a/b".into()], 0, 1).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].replacement, "x#y");
        let s = Substitute::parse("s/x//", None).unwrap();
        assert_eq!(s.edits(&["xx".into()], 0, 1).unwrap().len(), 1);
        let s = Substitute::parse(r"s/^/é/g", None).unwrap();
        let edits = s.edits(&["界".into(), "é".into()], 0, 2).unwrap();
        assert_eq!((edits[0].start, edits[0].end), (0, 0));
        assert_eq!(edits[1].start, 2);
    }
}
