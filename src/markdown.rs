use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

pub fn parse_markdown(content: &str) -> Text<'_> {
    let mut lines = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            let style = Style::default().fg(Color::DarkGray);
            lines.push(Line::from(Span::styled(line.to_string(), style)));
            continue;
        }

        if in_code_block {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Magenta),
            )));
            continue;
        }

        if let Some(rest) = line.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(
                format!("{} ", rest),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            )));
        } else if let Some(rest) = line.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if let Some(rest) = line.strip_prefix("### ") {
            lines.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Cyan)),
                Span::raw(rest.to_string()),
            ]));
        } else if let Some(rest) = line.strip_prefix("> ") {
            lines.push(Line::from(Span::styled(
                format!("  ┃ {}", rest),
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            // Basic inline styling: **bold**, `code`
            let mut spans = Vec::new();
            let mut current = line;

            while !current.is_empty() {
                let bold_start = current.find("**");
                let code_start = current.find("`").filter(|&i| {
                    // check if it's not part of ``` (which should be handled above)
                    !current[i..].starts_with("```")
                });

                match (bold_start, code_start) {
                    (Some(b), Some(c)) if b < c => {
                        spans.push(Span::raw(current[..b].to_string()));
                        if let Some(end) = current[b + 2..].find("**") {
                            spans.push(Span::styled(
                                current[b + 2..b + 2 + end].to_string(),
                                Style::default()
                                    .add_modifier(Modifier::BOLD)
                                    .fg(Color::LightYellow),
                            ));
                            current = &current[b + 2 + end + 2..];
                        } else {
                            spans.push(Span::raw("**"));
                            current = &current[b + 2..];
                        }
                    }
                    (_, Some(c)) => {
                        spans.push(Span::raw(current[..c].to_string()));
                        if let Some(end) = current[c + 1..].find("`") {
                            spans.push(Span::styled(
                                current[c + 1..c + 1 + end].to_string(),
                                Style::default()
                                    .bg(Color::Rgb(40, 44, 52))
                                    .fg(Color::LightCyan),
                            ));
                            current = &current[c + 1 + end + 1..];
                        } else {
                            spans.push(Span::raw("`"));
                            current = &current[c + 1..];
                        }
                    }
                    (Some(b), None) => {
                        spans.push(Span::raw(current[..b].to_string()));
                        if let Some(end) = current[b + 2..].find("**") {
                            spans.push(Span::styled(
                                current[b + 2..b + 2 + end].to_string(),
                                Style::default()
                                    .add_modifier(Modifier::BOLD)
                                    .fg(Color::LightYellow),
                            ));
                            current = &current[b + 2 + end + 2..];
                        } else {
                            spans.push(Span::raw("**"));
                            current = &current[b + 2..];
                        }
                    }
                    (None, None) => {
                        spans.push(Span::raw(current.to_string()));
                        break;
                    }
                }
            }
            lines.push(Line::from(spans));
        }
    }
    Text::from(lines)
}
