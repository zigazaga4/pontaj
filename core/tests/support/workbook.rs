//! The exported file read back: the constants and the formulas of every sheet (the values cached in the formula cells are
//! kept apart, to be compared with what the formulas really compute).

use std::collections::HashMap;
use std::path::Path;

use calamine::{open_workbook, Data, Reader, Xlsx};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    Num(f64),
    Text(String),
    Bool(bool),
    Error(String),
}

impl Value {
    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Nothing a reader would see: empty, or only spaces.
    pub fn is_blank(&self) -> bool {
        match self {
            Value::Empty => true,
            Value::Text(t) => t.trim().is_empty(),
            _ => false,
        }
    }

    /// A time or duration cell as whole minutes.
    pub fn minutes(&self) -> Option<i32> {
        self.num().map(|v| (v * 1440.0).round() as i32)
    }

    pub fn show(&self) -> String {
        match self {
            Value::Empty => "(gol)".into(),
            Value::Num(n) => format!("{n}"),
            Value::Text(t) => format!("{t:?}"),
            Value::Bool(b) => format!("{b}"),
            Value::Error(e) => format!("#{e}"),
        }
    }
}

pub struct Sheet {
    pub name: String,
    /// Constants, and for formula cells the value the file has cached.
    pub values: HashMap<(u32, u32), Value>,
    pub formulas: HashMap<(u32, u32), String>,
}

impl Sheet {
    pub fn value(&self, row: u32, col: u32) -> Value {
        self.values.get(&(row, col)).cloned().unwrap_or(Value::Empty)
    }

    pub fn text(&self, row: u32, col: u32) -> Option<String> {
        match self.values.get(&(row, col)) {
            Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.clone()),
            _ => None,
        }
    }
}

pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

impl Workbook {
    pub fn read(path: &Path) -> Workbook {
        let mut wb: Xlsx<_> = open_workbook(path).expect("the exported file opens");
        let names = wb.sheet_names();
        let mut sheets = Vec::new();
        for name in names {
            let range = wb.worksheet_range(&name).expect("a sheet of the file");
            let mut values = HashMap::new();
            if let Some((r0, c0)) = range.start() {
                for (r, c, v) in range.used_cells() {
                    values.insert((r0 + r as u32, c0 + c as u32), convert(v));
                }
            }
            let found = wb.worksheet_formula(&name).expect("the formulas of a sheet");
            let mut formulas = HashMap::new();
            if let Some((r0, c0)) = found.start() {
                for (r, c, text) in found.used_cells() {
                    if !text.trim().is_empty() {
                        formulas.insert((r0 + r as u32, c0 + c as u32), text.trim_start_matches('=').to_string());
                    }
                }
            }
            sheets.push(Sheet { name, values, formulas });
        }
        Workbook { sheets }
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name == name)
    }
}

fn convert(v: &Data) -> Value {
    match v {
        Data::Int(i) => Value::Num(*i as f64),
        Data::Float(f) => Value::Num(*f),
        Data::String(s) => Value::Text(s.clone()),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(d) => Value::Num(d.as_f64()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::Text(s.clone()),
        Data::Error(e) => Value::Error(format!("{e:?}")),
        Data::Empty => Value::Empty,
    }
}
