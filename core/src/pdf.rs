//! Reads tachograph "Protocol perioadă specială" PDF reports.
//!
//! The report is a table where empty cells are simply missing, so values are
//! mapped to columns by their horizontal position, using the table header to
//! locate each column.

use std::path::Path;

use chrono::NaiveDate;
use pdf_extract::{Document, MediaBox, OutputDev, OutputError, Transform};
use serde::{Deserialize, Serialize};

use crate::time::{parse_hm, DAY};
use crate::{Error, Result};

/// One row of the report: a continuous activity block on one calendar day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub date: NaiveDate,
    /// Minutes from midnight of `date`.
    pub start: i32,
    /// Minutes from midnight of `date`; `1440` when the block ends at 0:00.
    pub end: i32,
    pub span: i32,
    pub driving: i32,
    pub other_work: i32,
    pub availability: i32,
    pub rest: i32,
    pub day_work: i32,
    pub night_work: i32,
    /// Working time = driving + other work (the report's last "TOTAL" column).
    pub work: i32,
    /// Report file the row was read from (empty for rows added by hand).
    #[serde(default)]
    pub source: String,
    /// Added or edited by hand: the derived columns are recomputed from the typed ones.
    #[serde(default)]
    pub manual: bool,
}

impl Segment {
    /// A row typed by hand: working time is `other_work` for the whole block.
    pub fn manual(date: NaiveDate, start: i32, end: i32) -> Self {
        let mut seg = Segment {
            date,
            start,
            end,
            span: 0,
            driving: 0,
            other_work: end - start,
            availability: 0,
            rest: 0,
            day_work: 0,
            night_work: 0,
            work: 0,
            source: String::new(),
            manual: true,
        };
        seg.normalize();
        seg
    }

    /// Same report row, whichever file it came from or whether it was edited.
    pub fn same_row(&self, other: &Segment) -> bool {
        self.date == other.date
            && (self.start, self.end, self.driving, self.other_work, self.availability, self.rest, self.night_work)
                == (other.start, other.end, other.driving, other.other_work, other.availability, other.rest, other.night_work)
    }

    /// A block ending at or before its start runs past midnight; edited rows
    /// get their derived columns (duration, working time, day work) refreshed.
    pub fn normalize(&mut self) {
        if self.end <= self.start {
            self.end += DAY;
        }
        if self.manual {
            self.span = self.end - self.start;
            self.work = self.driving + self.other_work;
            self.day_work = (self.work - self.night_work).max(0);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverReport {
    /// Name as printed in the report, e.g. "Popescu, Ion".
    pub name: String,
    pub company: String,
    pub location: String,
    pub period: Option<(NaiveDate, NaiveDate)>,
    pub segments: Vec<Segment>,
    pub source: String,
}

/// Parses every driver found in a tachograph PDF.
pub fn parse_file(path: &Path) -> Result<Vec<DriverReport>> {
    let bytes = std::fs::read(path).map_err(|e| Error::new(format!("Nu pot citi {}: {e}", path.display())))?;
    let source = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    parse_bytes(&bytes, &source)
}

pub fn parse_bytes(bytes: &[u8], source: &str) -> Result<Vec<DriverReport>> {
    let doc = Document::load_mem(bytes).map_err(|e| Error::new(format!("{source}: PDF invalid ({e})")))?;
    let mut collector = Collector::default();
    pdf_extract::output_doc(&doc, &mut collector).map_err(|e| Error::new(format!("{source}: nu pot extrage textul ({e})")))?;
    let reports = parse_pages(collector.pages.iter().map(|g| lines_of(g)), source);
    if reports.is_empty() {
        return Err(Error::new(format!("{source}: nu pare a fi un raport de tahograf (lipsește „Numele șoferului”)")));
    }
    Ok(reports)
}

// ---------------------------------------------------------------------------
// Positioned text extraction
// ---------------------------------------------------------------------------

struct Glyph {
    x: f64,
    y: f64,
    advance: f64,
    text: String,
}

#[derive(Default)]
struct Collector {
    pages: Vec<Vec<Glyph>>,
}

impl OutputDev for Collector {
    fn begin_page(&mut self, _: u32, _: &MediaBox, _: Option<(f64, f64, f64, f64)>) -> std::result::Result<(), OutputError> {
        self.pages.push(Vec::new());
        Ok(())
    }
    fn end_page(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }
    fn output_character(&mut self, trm: &Transform, width: f64, _: f64, font_size: f64, text: &str) -> std::result::Result<(), OutputError> {
        if let Some(page) = self.pages.last_mut() {
            page.push(Glyph { x: trm.m31, y: trm.m32, advance: width * font_size, text: text.to_string() });
        }
        Ok(())
    }
    fn begin_word(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Word {
    text: String,
    x0: f64,
    x1: f64,
}

impl Word {
    fn center(&self) -> f64 {
        (self.x0 + self.x1) / 2.0
    }
}

#[derive(Debug)]
struct Line {
    words: Vec<Word>,
}

impl Line {
    fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// Groups glyphs into lines (same baseline) and words (separated by spaces or gaps).
fn lines_of(glyphs: &[Glyph]) -> Vec<Line> {
    const SAME_LINE: f64 = 2.0;
    const WORD_GAP: f64 = 1.5;

    let mut sorted: Vec<&Glyph> = glyphs.iter().collect();
    sorted.sort_by(|a, b| b.y.total_cmp(&a.y));

    let mut rows: Vec<Vec<&Glyph>> = Vec::new();
    for g in sorted {
        match rows.last_mut() {
            Some(row) if (row[0].y - g.y).abs() <= SAME_LINE => row.push(g),
            _ => rows.push(vec![g]),
        }
    }

    rows.into_iter()
        .map(|mut row| {
            row.sort_by(|a, b| a.x.total_cmp(&b.x));
            let mut words: Vec<Word> = Vec::new();
            let mut open = false;
            for g in row {
                if g.text.trim().is_empty() {
                    open = false;
                    continue;
                }
                match words.last_mut() {
                    Some(w) if open && g.x - w.x1 <= WORD_GAP => {
                        w.text.push_str(&g.text);
                        w.x1 = g.x + g.advance;
                    }
                    _ => words.push(Word { text: g.text.clone(), x0: g.x, x1: g.x + g.advance }),
                }
                open = true;
            }
            Line { words }
        })
        .filter(|l| !l.words.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// Report structure
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Col {
    Start,
    End,
    Span,
    Driving,
    OtherWork,
    Availability,
    Rest,
    DayWork,
    NightWork,
    Work,
}

/// Column centers (PDF points) as laid out by the report generator; refined
/// from the header line of each page when present.
fn default_columns() -> Vec<(Col, f64)> {
    vec![
        (Col::Start, 136.0),
        (Col::End, 182.0),
        (Col::Span, 228.0),
        (Col::Driving, 274.0),
        (Col::OtherWork, 320.0),
        (Col::Availability, 366.0),
        (Col::Rest, 412.0),
        (Col::DayWork, 458.0),
        (Col::NightWork, 504.0),
        (Col::Work, 550.0),
    ]
}

/// Header: `Data Start Final Total [4 activity icons] ZI NOAPTE TOTAL`.
fn columns_from_header(line: &Line) -> Option<Vec<(Col, f64)>> {
    let find = |t: &str| line.words.iter().find(|w| w.text == t).map(Word::center);
    let (start, end, span) = (find("Start")?, find("Final")?, find("Total")?);
    let (day, night, work) = (find("ZI")?, find("NOAPTE")?, find("TOTAL")?);
    let step = (day - span) / 5.0;
    Some(vec![
        (Col::Start, start),
        (Col::End, end),
        (Col::Span, span),
        (Col::Driving, span + step),
        (Col::OtherWork, span + 2.0 * step),
        (Col::Availability, span + 3.0 * step),
        (Col::Rest, span + 4.0 * step),
        (Col::DayWork, day),
        (Col::NightWork, night),
        (Col::Work, work),
    ])
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim_end_matches(','), "%d.%m.%Y").ok()
}

/// Text after the first `label` occurrence, up to `stop` (if present).
fn field<'a>(text: &'a str, label: &str, stop: Option<&str>) -> Option<&'a str> {
    let rest = &text[text.find(label)? + label.len()..];
    let rest = match stop.and_then(|s| rest.find(s)) {
        Some(i) => &rest[..i],
        None => rest,
    };
    Some(rest.trim())
}

/// A data row: `Mar, 01.09.2026 4:18 13:00 08:42 06:03 01:02 01:37 05:23 01:42 07:05`.
fn parse_row(line: &Line, columns: &[(Col, f64)]) -> Option<Segment> {
    let date_idx = line.words.iter().take(3).position(|w| parse_date(&w.text).is_some())?;
    let date = parse_date(&line.words[date_idx].text)?;
    let date_right = line.words[date_idx].x1;

    let mut seg = Segment {
        date,
        start: -1,
        end: -1,
        span: 0,
        driving: 0,
        other_work: 0,
        availability: 0,
        rest: 0,
        day_work: 0,
        night_work: 0,
        work: -1,
        source: String::new(),
        manual: false,
    };
    for w in line.words.iter().skip(date_idx + 1).filter(|w| w.x0 >= date_right) {
        let Some(value) = parse_hm(&w.text) else { continue };
        let center = w.center();
        let (col, _) = columns.iter().min_by(|a, b| (a.1 - center).abs().total_cmp(&(b.1 - center).abs()))?;
        let slot = match col {
            Col::Start => &mut seg.start,
            Col::End => &mut seg.end,
            Col::Span => &mut seg.span,
            Col::Driving => &mut seg.driving,
            Col::OtherWork => &mut seg.other_work,
            Col::Availability => &mut seg.availability,
            Col::Rest => &mut seg.rest,
            Col::DayWork => &mut seg.day_work,
            Col::NightWork => &mut seg.night_work,
            Col::Work => &mut seg.work,
        };
        *slot = value;
    }
    if seg.start < 0 || seg.end < 0 {
        return None;
    }
    if seg.end <= seg.start {
        seg.end += DAY;
    }
    if seg.work < 0 {
        seg.work = seg.driving + seg.other_work;
    }
    Some(seg)
}

fn parse_pages(pages: impl Iterator<Item = Vec<Line>>, source: &str) -> Vec<DriverReport> {
    let mut reports: Vec<DriverReport> = Vec::new();
    let mut company = String::new();
    let mut period = None;

    for lines in pages {
        let mut columns = default_columns();
        let mut location = String::new();
        let mut current: Option<usize> = None;

        for line in &lines {
            let text = line.text();
            if let Some(v) = field(&text, "NUME DE CONT:", Some("Perioada:")) {
                company = v.to_string();
            }
            if let Some(v) = field(&text, "Perioada:", None) {
                let dates: Vec<NaiveDate> = v.split_whitespace().filter_map(parse_date).collect();
                if let [from, to, ..] = dates[..] {
                    period = Some((from, to));
                }
            }
            if let Some(v) = field(&text, "sau grup:", None) {
                location = v.to_string();
            }
            if let Some(name) = field(&text, "oferului:", Some("Cod ")) {
                let name = name.trim_end_matches(':').trim().to_string();
                if name.is_empty() {
                    continue;
                }
                // A driver usually continues over several pages: reuse the open report.
                current = Some(match reports.iter().position(|r| r.name == name) {
                    Some(i) => i,
                    None => {
                        reports.push(DriverReport {
                            name,
                            company: company.clone(),
                            location: location.clone(),
                            period,
                            segments: Vec::new(),
                            source: source.to_string(),
                        });
                        reports.len() - 1
                    }
                });
                continue;
            }
            if text.contains("Start") && text.contains("NOAPTE") {
                if let Some(cols) = columns_from_header(line) {
                    columns = cols;
                }
                continue;
            }
            if let (Some(i), Some(mut seg)) = (current, parse_row(line, &columns)) {
                seg.source = source.to_string();
                let segments = &mut reports[i].segments;
                if !segments.contains(&seg) {
                    segments.push(seg);
                }
            }
        }
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(words: &[(&str, f64)]) -> Line {
        Line { words: words.iter().map(|(t, x)| Word { text: t.to_string(), x0: *x - 14.0, x1: *x + 14.0 }).collect() }
    }

    #[test]
    fn maps_values_to_columns_even_with_gaps() {
        // "Mie, 02.09.2026 6:50 17:09 10:19 06:28 01:01 02:50 07:29 07:29" — no night, no availability.
        let row = line(&[
            ("Mie,", 31.0),
            ("02.09.2026", 73.5),
            ("6:50", 136.0),
            ("17:09", 182.0),
            ("10:19", 228.0),
            ("06:28", 274.0),
            ("01:01", 320.0),
            ("02:50", 412.0),
            ("07:29", 458.0),
            ("07:29", 550.0),
        ]);
        let seg = parse_row(&row, &default_columns()).unwrap();
        assert_eq!((seg.start, seg.end), (410, 1029));
        assert_eq!((seg.driving, seg.other_work, seg.rest), (388, 61, 170));
        assert_eq!((seg.day_work, seg.night_work, seg.work), (449, 0, 449));
    }

    #[test]
    fn block_ending_at_midnight_runs_to_24h() {
        let row = line(&[("Dum,", 31.0), ("13.09.2026", 80.0), ("23:05", 136.0), ("0:00", 182.0), ("00:07", 504.0), ("00:07", 550.0)]);
        let seg = parse_row(&row, &default_columns()).unwrap();
        assert_eq!((seg.start, seg.end, seg.work), (23 * 60 + 5, DAY, 7));
    }

    #[test]
    fn weekly_summary_is_not_a_row() {
        let row = line(&[("3", 88.0), ("Zile", 104.0), ("28:46", 228.0), ("20:43", 550.0)]);
        assert!(parse_row(&row, &default_columns()).is_none());
    }
}
