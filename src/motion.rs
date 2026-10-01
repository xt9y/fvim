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

fn word(b: &Buffer, key: char, count: usize) -> usize {
    let length = b.body_len();
    if length == 0 {
        return 0;
    }
    let mut pos = b.offset().min(length - 1);
    let big = key.is_uppercase();
    for _ in 0..count {
        match key.to_ascii_lowercase() {
            'w' => {
                let mut chars = b.chars_forward(pos).peekable();
                let kind = class(*chars.peek().unwrap(), big);
                while chars.peek().is_some_and(|&ch| class(ch, big) == kind) {
                    chars.next();
                    pos += 1;
                }
                while chars.peek().is_some_and(|&ch| class(ch, big) == 0) {
                    chars.next();
                    pos += 1;
                }
                if pos == length {
                    return pos;
                }
            }
            'b' => {
                let mut chars = b.chars_backward(pos).peekable();
                let Some(mut ch) = chars.next() else {
                    return 0;
                };
                pos -= 1;
                while pos > 0 && class(ch, big) == 0 {
                    ch = chars.next().unwrap();
                    pos -= 1;
                }
                let kind = class(ch, big);
                while chars.peek().is_some_and(|&ch| class(ch, big) == kind) {
                    chars.next();
                    pos -= 1;
                }
            }
            'e' => {
                if pos + 1 < length {
                    pos += 1;
                }
                let mut chars = b.chars_forward(pos).peekable();
                while pos + 1 < length && chars.peek().is_some_and(|&ch| class(ch, big) == 0) {
                    chars.next();
                    pos += 1;
                }
                let kind = class(chars.next().unwrap(), big);
                while chars.peek().is_some_and(|&ch| class(ch, big) == kind) {
                    chars.next();
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
            word(b, key, count)
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
            let start = b.offset();
            let (index, ch) = b
                .chars_forward(start)
                .take(length.saturating_sub(b.col))
                .enumerate()
                .find(|&(_, ch)| pairs(ch).is_some())?;
            let (open, close) = pairs(ch)?;
            inclusive = true;
            matching(b, start + index, open, close)?
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

fn matching(b: &Buffer, pos: usize, open: char, close: char) -> Option<usize> {
    fn scan(chars: impl Iterator<Item = (usize, char)>, open: char, close: char) -> Option<usize> {
        let mut depth = 0usize;
        for (i, ch) in chars {
            if ch == open {
                depth += 1;
            }
            if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        None
    }
    if b.chars_forward(pos).next()? == open {
        scan((pos..).zip(b.chars_forward(pos)), open, close)
    } else {
        scan((0..=pos).rev().zip(b.chars_backward(pos + 1)), close, open)
    }
}

pub fn object(b: &Buffer, key: char, around: bool, count: usize) -> Option<(usize, usize)> {
    let pos = b.offset();
    let current = b.chars_forward(pos).next()?;
    if matches!(key, 'w' | 'W') {
        let big = key == 'W';
        let kind = class(current, big);
        let mut start = pos
            - b.chars_backward(pos)
                .take_while(|&ch| class(ch, big) == kind)
                .count();
        let mut end = pos
            + b.chars_forward(pos)
                .take_while(|&ch| class(ch, big) == kind)
                .count();
        let mut chars = b.chars_forward(end).peekable();
        for _ in 1..count {
            while chars.peek().is_some_and(|&ch| class(ch, big) == 0) {
                chars.next();
                end += 1;
            }
            let Some(&ch) = chars.peek() else {
                break;
            };
            let next = class(ch, big);
            while chars.peek().is_some_and(|&ch| class(ch, big) == next) {
                chars.next();
                end += 1;
            }
        }
        if around {
            let old = end;
            end += b
                .chars_forward(end)
                .take_while(|&ch| ch.is_whitespace() && ch != '\n')
                .count();
            if end == old {
                start -= b
                    .chars_backward(start)
                    .take_while(|&ch| ch.is_whitespace() && ch != '\n')
                    .count();
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
        for (index, ch) in b.chars_backward(pos + 1).enumerate() {
            let i = pos - index;
            if ch == open {
                if let Some(j) = matching(b, i, open, close) {
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
        let base = b.offset_at(b.row, 0);
        let (mut previous, mut opening, mut found) = ('\0', None, None);
        for (col, ch) in b.lines[b.row].chars().enumerate() {
            if ch == key && previous != '\\' {
                if let Some(start) = opening.take() {
                    if start <= b.col && b.col <= col {
                        found = Some((base + start, base + col));
                        break;
                    }
                } else {
                    opening = Some(col);
                }
            }
            previous = ch;
        }
        found?
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
