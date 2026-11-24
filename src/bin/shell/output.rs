use std::io::{self, Write};
use std::str::FromStr;
use triplelight::exec::{QueryResult, results};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy)]
pub(super) enum Mode {
    Box,
    Tsv,
    Csv,
    Json,
}

impl FromStr for Mode {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "box" => Ok(Self::Box),
            "tsv" => Ok(Self::Tsv),
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            _ => Err("output mode must be box, tsv, csv or json"),
        }
    }
}

impl Mode {
    pub(super) fn write(self, result: &QueryResult, out: &mut impl Write) -> io::Result<()> {
        if matches!(self, Self::Box) {
            match result {
                QueryResult::Solutions { vars, rows } => {
                    return write_box(
                        out,
                        vars.iter()
                            .map(|var| format!("?{var_name}", var_name = var.as_str())),
                        rows,
                    );
                }
                QueryResult::Boolean(value) => return writeln!(out, "{value}"),
                _ => {}
            }
        }
        let (_, body) = match self {
            Self::Tsv => results::serialize_tsv(result),
            Self::Csv => results::serialize_csv(result),
            _ => None,
        }
        .unwrap_or_else(|| results::serialize(result));
        out.write_all(body.as_bytes())?;
        if !body.is_empty() && !body.ends_with('\n') {
            writeln!(out)?;
        }
        Ok(())
    }
}

const DISPLAY_ROWS: usize = 40;
const CELL_WIDTH: usize = 44;

fn write_box(
    out: &mut impl Write,
    headers: impl Iterator<Item = String>,
    rows: &[Vec<Option<triplelight_rdf::Term>>],
) -> io::Result<()> {
    let headers: Vec<_> = headers.map(cell).collect();
    if !headers.is_empty() {
        let mut widths: Vec<_> = headers.iter().map(|text| text.as_str().width()).collect();
        let preview: Vec<Vec<String>> = rows
            .iter()
            .take(DISPLAY_ROWS)
            .map(|row| {
                row.iter()
                    .zip(&mut widths)
                    .map(|(term, width)| {
                        let text = term
                            .as_ref()
                            .map_or_else(|| "UNBOUND".into(), |t| cell(t.to_string()));
                        *width = (*width).max(text.as_str().width());
                        text
                    })
                    .collect()
            })
            .collect();

        border(out, &widths)?;
        write_row(out, &headers, &widths)?;
        border(out, &widths)?;
        for row in &preview {
            write_row(out, row, &widths)?;
        }
        if !preview.is_empty() {
            border(out, &widths)?;
        }
    }
    if rows.len() > DISPLAY_ROWS && !headers.is_empty() {
        writeln!(out, "({} rows, showing first {DISPLAY_ROWS})", rows.len())
    } else {
        writeln!(
            out,
            "({} {})",
            rows.len(),
            if rows.len() == 1 { "row" } else { "rows" }
        )
    }
}

fn border(out: &mut impl Write, widths: &[usize]) -> io::Result<()> {
    write!(out, "+")?;
    for width in widths {
        write!(out, "{:-<width$}+", "", width = width + 2)?;
    }
    writeln!(out)
}

fn write_row(out: &mut impl Write, cells: &[String], widths: &[usize]) -> io::Result<()> {
    write!(out, "|")?;
    for (text, width) in cells.iter().zip(widths) {
        let padding = width - text.as_str().width();
        write!(out, " {text}{:padding$} |", "")?;
    }
    writeln!(out)
}

fn cell(text: String) -> String {
    let text = if text.chars().any(char::is_control) {
        let mut escaped = String::with_capacity(text.len());
        for c in text.chars() {
            if c.is_control() {
                escaped.extend(c.escape_default());
            } else {
                escaped.push(c);
            }
        }
        escaped
    } else {
        text
    };
    if text.as_str().width() <= CELL_WIDTH {
        return text;
    }
    let mut remaining = CELL_WIDTH - 1;
    let mut short = String::new();
    for c in text.chars() {
        let width = c.width().unwrap_or(0);
        if width > remaining {
            break;
        }
        remaining -= width;
        short.push(c);
    }
    short.push('…');
    short
}
