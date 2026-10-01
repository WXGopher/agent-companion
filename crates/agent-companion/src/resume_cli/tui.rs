//! Terminal ownership is scoped so launch, cancellation, and errors restore it.
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, Write};
use unicode_width::UnicodeWidthChar;

pub struct Item {
    pub title: String,
    pub search: String,
    pub summary: Vec<String>,
    pub details: Vec<String>,
    pub blockers: Vec<String>,
}

struct Terminal;
impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(guard)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

pub fn choose(title: &str, subtitle: &str, items: &[Item]) -> io::Result<Option<usize>> {
    let _terminal = Terminal::enter()?;
    let mut query = String::new();
    let mut selected = 0usize;
    let mut expanded = false;
    let mut detail_offset = 0;
    let mut note = String::new();
    loop {
        let matches = filtered(items, &query);
        selected = selected.min(matches.len().saturating_sub(1));
        render(
            title,
            subtitle,
            items,
            &matches,
            selected,
            &query,
            expanded,
            detail_offset,
            &note,
        )?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                detail_offset = 0;
            }
            KeyCode::Down => {
                selected = (selected + 1).min(matches.len().saturating_sub(1));
                detail_offset = 0;
            }
            KeyCode::Tab => {
                expanded = !expanded;
                detail_offset = 0;
            }
            KeyCode::PageDown if expanded => detail_offset = detail_offset.saturating_add(5),
            KeyCode::PageUp if expanded => detail_offset = detail_offset.saturating_sub(5),
            KeyCode::Enter => {
                if let Some(&index) = matches.get(selected) {
                    if items[index].blockers.is_empty() {
                        return Ok(Some(index));
                    }
                    note = format!("Disabled: {}", items[index].blockers.join("; "));
                }
            }
            KeyCode::Backspace => {
                query.pop();
                selected = 0;
                detail_offset = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if query.len() < 1024 {
                    query.push(character);
                }
                selected = 0;
                detail_offset = 0;
            }
            _ => (),
        }
    }
}

fn filtered(items: &[Item], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| item.search.to_lowercase().contains(&query).then_some(index))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render(
    title: &str,
    subtitle: &str,
    items: &[Item],
    matches: &[usize],
    selected: usize,
    query: &str,
    expanded: bool,
    detail_offset: usize,
    note: &str,
) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let mut output = io::stdout().lock();
    queue!(output, MoveTo(0, 0), Clear(ClearType::All))?;
    let mut lines = vec![
        title.to_owned(),
        subtitle.to_owned(),
        "Type to search · ↑↓ select · Enter continue · Tab details · PgUp/PgDn details · Esc back"
            .into(),
        format!("Search: {query}   ({} matches)", matches.len()),
        String::new(),
    ];
    let list_height = (usize::from(height).saturating_sub(14) / 2).clamp(1, 10);
    let start = selected.saturating_sub(list_height.saturating_sub(1));
    for (position, &index) in matches.iter().enumerate().skip(start).take(list_height) {
        let item = &items[index];
        lines.push(format!(
            "{} {}{}",
            if position == selected { "›" } else { " " },
            item.title,
            if item.blockers.is_empty() {
                ""
            } else {
                "  [disabled]"
            }
        ));
    }
    if matches.is_empty() {
        lines.push("No matching items.".into());
    }
    lines.push(String::new());
    if let Some(&index) = matches.get(selected) {
        let item = &items[index];
        lines.extend(item.summary.iter().cloned());
        if expanded {
            lines.push("Details (PgUp/PgDn scroll):".into());
            let details: Vec<_> = item
                .details
                .iter()
                .cloned()
                .chain(
                    item.blockers
                        .iter()
                        .map(|reason| format!("Disabled: {reason}")),
                )
                .flat_map(|line| wrapped(&line, usize::from(width.saturating_sub(1))))
                .collect();
            let offset = detail_offset.min(details.len().saturating_sub(1));
            lines.extend(details.into_iter().skip(offset));
        }
        if !item.blockers.is_empty() {
            lines.push(format!("Disabled: {}", item.blockers.join("; ")));
        }
    }
    let content_height = usize::from(height.saturating_sub(1));
    for (row, line) in lines.iter().take(content_height).enumerate() {
        queue!(output, MoveTo(0, row as u16))?;
        if row == 0 {
            queue!(output, SetAttribute(Attribute::Bold))?;
        }
        queue!(
            output,
            Print(clipped(line, usize::from(width.saturating_sub(1)))),
            SetAttribute(Attribute::Reset)
        )?;
    }
    let blocker = matches
        .get(selected)
        .and_then(|index| items[*index].blockers.first())
        .map(|reason| format!("Disabled: {reason}"));
    let footer = blocker.as_deref().unwrap_or(note);
    if !footer.is_empty() && height > 0 {
        queue!(
            output,
            MoveTo(0, height - 1),
            Print(clipped(footer, usize::from(width.saturating_sub(1))))
        )?;
    }
    output.flush()
}

/// Rollout titles and filenames are untrusted terminal text, never control codes.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}
fn clipped(text: &str, width: usize) -> String {
    let mut used = 0;
    clean(text)
        .chars()
        .take_while(|character| {
            used += character.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut result = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for character in clean(text).chars() {
        let size = character.width().unwrap_or(0);
        if used + size > width && !line.is_empty() {
            result.push(std::mem::take(&mut line));
            used = 0;
        }
        if size > width {
            line.push('?');
            used += 1;
        } else {
            line.push(character);
            used += size;
        }
    }
    result.push(line);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_is_case_insensitive_and_includes_identity() {
        let items = [Item {
            title: "work".into(),
            search: "work DODEX abc-123".into(),
            summary: vec![],
            details: vec![],
            blockers: vec![],
        }];
        assert_eq!(filtered(&items, "dodex"), [0]);
        assert_eq!(filtered(&items, "123"), [0]);
        assert!(filtered(&items, "missing").is_empty());
        assert_eq!(clean("title\n\x1b[2J"), "title  [2J");
        assert_eq!(clipped("中文abc", 5), "中文a");
        assert_eq!(wrapped("中文abc", 5), ["中文a", "bc"]);
    }
}
