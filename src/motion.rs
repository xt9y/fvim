use crate::buffer::Buffer;

#[derive(Clone, Copy)]
pub struct Motion {
    pub offset: usize,
    pub inclusive: bool,
    pub linewise: bool,
}

fn class(ch: char, big: bool) -> u8 {
    if ch.is_whitespace() {
        0
    } else if big || ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

pub fn word(chars: &[char], mut pos: usize, key: char, count: usize) -> usize {
    if chars.is_empty() {
        return 0;
    }
    pos = pos.min(chars.len() - 1);
    let big = key.is_uppercase();
    for _ in 0..count {
        match key.to_ascii_lowercase() {
            'w' => {
                let kind = class(chars[pos], big);
                while pos < chars.len() && class(chars[pos], big) == kind {
                    pos += 1;
                }
                while pos < chars.len() && class(chars[pos], big) == 0 {
                    pos += 1;
                }
                if pos == chars.len() {
                    return pos;
                }
            }
            'b' => {
                pos = pos.saturating_sub(1);
                while pos > 0 && class(chars[pos], big) == 0 {
                    pos -= 1;
                }
                let kind = class(chars[pos], big);
                while pos > 0 && class(chars[pos - 1], big) == kind {
                    pos -= 1;
                }
            }
            'e' => {
                if pos + 1 < chars.len() {
                    pos += 1;
                }
                while pos + 1 < chars.len() && class(chars[pos], big) == 0 {
                    pos += 1;
                }
                let kind = class(chars[pos], big);
                while pos + 1 < chars.len() && class(chars[pos + 1], big) == kind {
                    pos += 1;
                }
            }
            _ => unreachable!(),
        }
    }
    pos
}

pub fn motion(b: &Buffer, key: char, count: usize, explicit: bool) -> Option<Motion> {
    let count = count.max(1);
    let length = b.lines[b.row].chars().count();
    let mut row = b.row;
    let mut col = b.col;
    let mut inclusive = false;
    let mut linewise = false;
    let offset = match key {
        'w' | 'W' | 'b' | 'B' | 'e' | 'E' => {
            inclusive = matches!(key, 'e' | 'E');
            word(
                &b.body().chars().collect::<Vec<_>>(),
                b.offset(),
                key,
                count,
            )
        }
        '%' if explicit => {
            row = (b
                .lines
                .len()
                .saturating_mul(count.min(100))
                .saturating_add(99)
                / 100)
                .saturating_sub(1);
            col = first_nonblank(&b.lines[row]);
            linewise = true;
            b.offset_at(row, col)
        }
        '%' => {
            let chars: Vec<_> = b.body().chars().collect();
            let start = b.offset();
            let line_end = b.offset_at(b.row, length);
            let pos = (start..line_end).find(|&i| pairs(chars[i]).is_some())?;
            let (open, close) = pairs(chars[pos])?;
            inclusive = true;
            matching(&chars, pos, open, close)?
        }
        '{' | '}' => {
            for _ in 0..count {
                let forward = key == '}';
                if forward {
                    if row + 1 >= b.lines.len() {
                        break;
                    }
                    row += 1;
                    while row + 1 < b.lines.len() && !b.lines[row].is_empty() {
                        row += 1;
                    }
                } else {
                    if row == 0 {
                        break;
                    }
                    row -= 1;
                    while row > 0 && !b.lines[row].is_empty() {
                        row -= 1;
                    }
                }
            }
            b.offset_at(row, 0)
        }
        _ => {
            match key {
                'h' => col = col.saturating_sub(count),
                'l' => col = col.saturating_add(count).min(length),
                'j' | 'k' => {
                    row = if key == 'j' {
                        row.saturating_add(count).min(b.lines.len() - 1)
                    } else {
                        row.saturating_sub(count)
                    };
                    col = col.min(b.lines[row].chars().count().saturating_sub(1));
                    linewise = true;
                }
                '0' => col = 0,
                '^' => col = first_nonblank(&b.lines[row]),
                '$' => {
                    row = row.saturating_add(count - 1).min(b.lines.len() - 1);
                    col = b.lines[row].chars().count().saturating_sub(1);
                    inclusive = true;
                }
                'g' | 'G' => {
                    row = if explicit {
                        count.saturating_sub(1).min(b.lines.len() - 1)
                    } else if key == 'g' {
                        0
                    } else {
                        b.lines.len() - 1
                    };
                    col = first_nonblank(&b.lines[row]);
                    linewise = true;
                }
                '+' | '-' => {
                    row = if key == '+' {
                        row.saturating_add(count).min(b.lines.len() - 1)
                    } else {
                        row.saturating_sub(count)
                    };
                    col = first_nonblank(&b.lines[row]);
                    linewise = true;
                }
                _ => return None,
            }
            b.offset_at(row, col)
        }
    };
    Some(Motion {
        offset,
        inclusive,
        linewise,
    })
}

pub fn first_nonblank(line: &str) -> usize {
    line.chars().position(|ch| !ch.is_whitespace()).unwrap_or(0)
}

pub fn find(b: &Buffer, key: char, target: char, count: usize) -> Option<Motion> {
    let chars: Vec<_> = b.lines[b.row].chars().collect();
    let forward = matches!(key, 'f' | 't');
    let pos = if forward {
        (b.col + 1..chars.len())
            .filter(|&i| chars[i] == target)
            .nth(count - 1)?
    } else {
        (0..b.col)
            .rev()
            .filter(|&i| chars[i] == target)
            .nth(count - 1)?
    };
    let pos = match key {
        't' => pos - 1,
        'T' => pos + 1,
        _ => pos,
    };
    Some(Motion {
        offset: b.offset_at(b.row, pos),
        inclusive: forward,
        linewise: false,
    })
}

fn pairs(ch: char) -> Option<(char, char)> {
    match ch {
        '(' | ')' => Some(('(', ')')),
        '[' | ']' => Some(('[', ']')),
        '{' | '}' => Some(('{', '}')),
        _ => None,
    }
}

fn matching(chars: &[char], pos: usize, open: char, close: char) -> Option<usize> {
    let forward = chars[pos] == open;
    let mut depth = 0usize;
    let indices: Box<dyn Iterator<Item = usize>> = if forward {
        Box::new(pos..chars.len())
    } else {
        Box::new((0..=pos).rev())
    };
    for i in indices {
        if chars[i] == if forward { open } else { close } {
            depth += 1;
        }
        if chars[i] == if forward { close } else { open } {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

pub fn object(b: &Buffer, key: char, around: bool, count: usize) -> Option<(usize, usize)> {
    let chars: Vec<_> = b.body().chars().collect();
    let pos = b.offset();
    if pos >= chars.len() {
        return None;
    }
    if matches!(key, 'w' | 'W') {
        let big = key == 'W';
        let kind = class(chars[pos], big);
        let mut start = pos;
        let mut end = pos + 1;
        while start > 0 && class(chars[start - 1], big) == kind {
            start -= 1;
        }
        while end < chars.len() && class(chars[end], big) == kind {
            end += 1;
        }
        for _ in 1..count {
            while end < chars.len() && class(chars[end], big) == 0 {
                end += 1;
            }
            if end == chars.len() {
                break;
            }
            let next = class(chars[end], big);
            while end < chars.len() && class(chars[end], big) == next {
                end += 1;
            }
        }
        if around {
            let old = end;
            while end < chars.len() && chars[end].is_whitespace() && chars[end] != '\n' {
                end += 1;
            }
            if end == old {
                while start > 0 && chars[start - 1].is_whitespace() && chars[start - 1] != '\n' {
                    start -= 1;
                }
            }
        }
        return Some((start, end));
    }
    let key = match key {
        'b' => '(',
        'B' => '{',
        _ => key,
    };
    let (start, end) = if let Some((open, close)) = pairs(key) {
        let mut found = None;
        let mut left = 0;
        for i in (0..=pos).rev() {
            if chars[i] == open {
                if let Some(j) = matching(&chars, i, open, close) {
                    if j >= pos {
                        left += 1;
                        if left == count {
                            found = Some((i, j));
                            break;
                        }
                    }
                }
            }
        }
        found?
    } else if matches!(key, '\'' | '"' | '`') {
        let start = b.offset_at(b.row, 0);
        let end = b.offset_at(b.row, b.lines[b.row].chars().count());
        let quotes: Vec<_> = (start..end)
            .filter(|&i| chars[i] == key && (i == 0 || chars[i - 1] != '\\'))
            .collect();
        quotes
            .as_chunks::<2>()
            .0
            .iter()
            .find(|pair| pair[0] <= pos && pos <= pair[1])
            .map(|pair| (pair[0], pair[1]))?
    } else {
        return None;
    };
    if around {
        Some((start, end + 1))
    } else {
        Some((start + 1, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    #[test]
    fn words_counts_and_unicode_offsets() {
        let b = Buffer::from_text("éone, two\nlast");
        assert_eq!(motion(&b, 'w', 1, false).unwrap().offset, 4);
        assert_eq!(motion(&b, 'w', 2, false).unwrap().offset, 6);
        assert_eq!(motion(&b, 'W', 1, false).unwrap().offset, 6);
        assert_eq!(motion(&b, 'e', 1, false).unwrap().offset, 3);
        let mut b = b;
        b.set_offset(7);
        assert_eq!(motion(&b, 'b', 1, false).unwrap().offset, 6);
        assert_eq!(motion(&b, 'B', 2, false).unwrap().offset, 0);
    }

    #[test]
    fn line_and_paragraph_motions() {
        let mut b = Buffer::from_text("  one\ntwo\n\nlast\n");
        assert_eq!(motion(&b, '^', 1, false).unwrap().offset, 2);
        assert_eq!(motion(&b, '$', 1, false).unwrap().offset, 4);
        assert_eq!(motion(&b, 'G', 1, false).unwrap().offset, 11);
        assert_eq!(motion(&b, 'G', 2, true).unwrap().offset, 6);
        assert_eq!(motion(&b, '}', 1, false).unwrap().offset, 10);
        b.set_offset(11);
        assert_eq!(motion(&b, '{', 1, false).unwrap().offset, 10);
    }

    #[test]
    fn nested_pairs_and_find_are_inclusive() {
        let mut b = Buffer::from_text("(a(b)c) xyz xyz");
        assert_eq!(motion(&b, '%', 1, false).unwrap().offset, 6);
        assert_eq!(find(&b, 'f', 'x', 2).unwrap().offset, 12);
        assert_eq!(find(&b, 't', 'x', 1).unwrap().offset, 7);
        b.col = 6;
        assert_eq!(motion(&b, '%', 1, false).unwrap().offset, 0);
        assert_eq!(find(&b, 'F', 'b', 1).unwrap().offset, 3);
    }

    #[test]
    fn word_quote_and_nested_text_objects() {
        let mut b = Buffer::from_text("one  two (a (inner) z) \"hello\"");
        b.col = 6;
        assert_eq!(object(&b, 'w', false, 1), Some((5, 8)));
        assert_eq!(object(&b, 'w', true, 1), Some((5, 9)));
        b.col = 14;
        assert_eq!(object(&b, '(', false, 1), Some((13, 18)));
        b.col = 25;
        assert_eq!(object(&b, '"', true, 1), Some((23, 30)));
        assert_eq!(object(&b, '[', false, 1), None);
    }
}
