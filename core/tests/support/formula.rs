//! A small Excel formula engine for what the export writes: IF, ROUND, INT, MOD, SUM, SUMIF(S), COUNTIF, MAX, MIN, DATE,
//! arithmetic, `&` and comparisons, with references to other sheets. It recomputes every formula of the file from the
//! constants, independently of the code that wrote the cached values, so a test sees what Excel shows when it opens the file.

use std::cell::RefCell;
use std::collections::HashMap;

use chrono::NaiveDate;

use super::workbook::{Value, Workbook};

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Cell { sheet: Option<String>, row: u32, col: u32 },
    Range { sheet: Option<String>, from: (u32, u32), to: (u32, u32) },
    Neg(Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

// ---------------------------------------------------------------------------------------------------------------------
// Reading a formula
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Str(String),
    /// A reference: optional sheet, then "A1" or "A1:B2".
    Ref(Option<String>, String),
    Func(String),
    Name(String),
    Op(String),
    Open,
    Close,
    Comma,
}

fn is_cell(text: &str) -> bool {
    let t = text.replace('$', "");
    let letters = t.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    letters > 0 && letters <= 3 && t.len() > letters && t[letters..].chars().all(|c| c.is_ascii_digit())
}

fn cell_pos(text: &str) -> (u32, u32) {
    let t = text.replace('$', "").to_ascii_uppercase();
    let letters: String = t.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let col = letters.chars().fold(0u32, |acc, c| acc * 26 + (c as u32 - 'A' as u32 + 1)) - 1;
    let row: u32 = t[letters.len()..].parse::<u32>().expect("row number") - 1;
    (row, col)
}

fn tokens(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // A reference after its sheet: "A1" or "A1:B2".
    let read_ref = |i: &mut usize| -> String {
        let start = *i;
        while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '$' || chars[*i] == ':') {
            *i += 1;
        }
        chars[start..*i].iter().collect()
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' => i += 1,
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    if chars[i] == '"' {
                        if i + 1 < chars.len() && chars[i + 1] == '"' {
                            s.push('"');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                out.push(Token::Str(s));
            }
            '\'' => {
                let mut sheet = String::new();
                i += 1;
                loop {
                    if chars[i] == '\'' {
                        if i + 1 < chars.len() && chars[i + 1] == '\'' {
                            sheet.push('\'');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    sheet.push(chars[i]);
                    i += 1;
                }
                assert_eq!(chars[i], '!', "a quoted sheet name is followed by '!' in {src}");
                i += 1;
                out.push(Token::Ref(Some(sheet), read_ref(&mut i)));
            }
            '(' => {
                out.push(Token::Open);
                i += 1;
            }
            ')' => {
                out.push(Token::Close);
                i += 1;
            }
            ',' | ';' => {
                out.push(Token::Comma);
                i += 1;
            }
            '<' | '>' => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                if two == "<=" || two == ">=" || two == "<>" {
                    out.push(Token::Op(two));
                    i += 2;
                } else {
                    out.push(Token::Op(c.to_string()));
                    i += 1;
                }
            }
            '=' | '+' | '-' | '*' | '/' | '&' | '^' => {
                out.push(Token::Op(c.to_string()));
                i += 1;
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'E' || chars[i] == 'e') {
                    i += 1;
                    if chars[i] == '+' || chars[i] == '-' {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                out.push(Token::Num(text.parse().expect("a number")));
            }
            _ if c.is_alphabetic() || c == '$' || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '$' || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if i < chars.len() && chars[i] == '!' {
                    i += 1;
                    out.push(Token::Ref(Some(word), read_ref(&mut i)));
                } else if i < chars.len() && chars[i] == '(' {
                    out.push(Token::Func(word.to_ascii_uppercase()));
                } else if is_cell(&word) {
                    let mut text = word;
                    if i < chars.len() && chars[i] == ':' {
                        i += 1;
                        text.push(':');
                        text.push_str(&read_ref(&mut i));
                    }
                    out.push(Token::Ref(None, text));
                } else {
                    out.push(Token::Name(word.to_ascii_uppercase()));
                }
            }
            _ => panic!("unexpected '{c}' in formula {src}"),
        }
    }
    out
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Token {
        self.at += 1;
        self.tokens[self.at - 1].clone()
    }

    fn op_is(&self, ops: &[&str]) -> Option<String> {
        match self.peek() {
            Some(Token::Op(op)) if ops.contains(&op.as_str()) => Some(op.clone()),
            _ => None,
        }
    }

    fn binary(&mut self, ops: &[&str], next: fn(&mut Parser) -> Expr) -> Expr {
        let mut left = next(self);
        while let Some(op) = self.op_is(ops) {
            self.at += 1;
            let right = next(self);
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        left
    }

    fn expr(&mut self) -> Expr {
        self.binary(&["=", "<>", "<", ">", "<=", ">="], Parser::concat)
    }

    fn concat(&mut self) -> Expr {
        self.binary(&["&"], Parser::additive)
    }

    fn additive(&mut self) -> Expr {
        self.binary(&["+", "-"], Parser::term)
    }

    fn term(&mut self) -> Expr {
        self.binary(&["*", "/"], Parser::power)
    }

    fn power(&mut self) -> Expr {
        self.binary(&["^"], Parser::unary)
    }

    fn unary(&mut self) -> Expr {
        match self.op_is(&["-", "+"]) {
            Some(op) => {
                self.at += 1;
                let inner = self.unary();
                if op == "-" { Expr::Neg(Box::new(inner)) } else { inner }
            }
            None => self.primary(),
        }
    }

    fn primary(&mut self) -> Expr {
        match self.next() {
            Token::Num(n) => Expr::Num(n),
            Token::Str(s) => Expr::Str(s),
            Token::Name(n) if n == "TRUE" => Expr::Bool(true),
            Token::Name(n) if n == "FALSE" => Expr::Bool(false),
            Token::Ref(sheet, text) => match text.split_once(':') {
                Some((a, b)) => Expr::Range { sheet, from: cell_pos(a), to: cell_pos(b) },
                None => {
                    let (row, col) = cell_pos(&text);
                    Expr::Cell { sheet, row, col }
                }
            },
            Token::Func(name) => {
                assert_eq!(self.next(), Token::Open);
                let mut args = Vec::new();
                if self.peek() != Some(&Token::Close) {
                    loop {
                        args.push(self.expr());
                        match self.next() {
                            Token::Comma => continue,
                            Token::Close => break,
                            other => panic!("unexpected {other:?} in the arguments of {name}"),
                        }
                    }
                } else {
                    self.at += 1;
                }
                Expr::Call(name, args)
            }
            Token::Open => {
                let inner = self.expr();
                assert_eq!(self.next(), Token::Close);
                inner
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn parse(src: &str) -> Expr {
    let mut parser = Parser { tokens: tokens(src), at: 0 };
    let e = parser.expr();
    assert!(parser.at == parser.tokens.len(), "formula not read to the end: {src}");
    e
}

// ---------------------------------------------------------------------------------------------------------------------
// Computing
// ---------------------------------------------------------------------------------------------------------------------

pub struct Engine<'a> {
    wb: &'a Workbook,
    memo: RefCell<HashMap<(usize, u32, u32), Value>>,
}

fn number(v: &Value) -> Result<f64, String> {
    match v {
        Value::Num(n) => Ok(*n),
        Value::Empty => Ok(0.0),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Text(t) => t.trim().parse::<f64>().map_err(|_| "VALUE".to_string()),
        Value::Error(e) => Err(e.clone()),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Num(n) => format!("{n}"),
        Value::Text(t) => t.clone(),
        Value::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).into(),
        Value::Empty | Value::Error(_) => String::new(),
    }
}

/// Excel's ROUND: halves away from zero (after the 15 significant digits Excel keeps).
fn excel_round(x: f64, digits: i32) -> f64 {
    let x = format!("{x:.12e}").parse::<f64>().unwrap_or(x);
    let f = 10f64.powi(digits);
    let r = (x.abs() * f + 0.5).floor() / f;
    if x < 0.0 { -r } else { r }
}

fn serial(date: NaiveDate) -> f64 {
    (date - NaiveDate::from_ymd_opt(1899, 12, 30).unwrap()).num_days() as f64
}

/// Excel's comparison: numbers before text before booleans; text without regard to case; empty = 0 or "".
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let rank = |v: &Value| match v {
        Value::Num(_) => 0,
        Value::Text(_) => 1,
        Value::Bool(_) => 2,
        _ => 0,
    };
    let (a, b) = match (a, b) {
        (Value::Empty, Value::Text(_)) => (Value::Text(String::new()), b.clone()),
        (Value::Text(_), Value::Empty) => (a.clone(), Value::Text(String::new())),
        (Value::Empty, _) => (Value::Num(0.0), b.clone()),
        (_, Value::Empty) => (a.clone(), Value::Num(0.0)),
        _ => (a.clone(), b.clone()),
    };
    match (&a, &b) {
        (Value::Num(x), Value::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => rank(&a).cmp(&rank(&b)),
    }
}

/// The criterion of SUMIF(S)/COUNTIF: "Staționare", "<>Completare", ">=5", or a number (a DATE(...)).
fn meets(cell: &Value, criterion: &Value) -> bool {
    if let Value::Num(n) = criterion {
        return matches!(cell, Value::Num(x) if (x - n).abs() < 1e-9);
    }
    let crit = text(criterion);
    let (op, operand) = ["<>", "<=", ">=", "<", ">", "="]
        .iter()
        .find_map(|op| crit.strip_prefix(op).map(|rest| (*op, rest.to_string())))
        .unwrap_or(("=", crit.clone()));
    if let Ok(n) = operand.trim().parse::<f64>() {
        return match (op, cell) {
            ("<>", Value::Num(x)) => (x - n).abs() >= 1e-9,
            ("<>", _) => true,
            (_, Value::Num(x)) => match op {
                "=" => (x - n).abs() < 1e-9,
                "<" => *x < n,
                ">" => *x > n,
                "<=" => *x <= n,
                ">=" => *x >= n,
                _ => false,
            },
            _ => false,
        };
    }
    let cell_text = match cell {
        Value::Empty => String::new(),
        other => text(other),
    };
    let equal = cell_text.to_lowercase() == operand.to_lowercase();
    match op {
        "=" => equal && !(operand.is_empty() && !matches!(cell, Value::Empty | Value::Text(_))),
        "<>" => !equal,
        _ => false,
    }
}

impl<'a> Engine<'a> {
    pub fn new(wb: &'a Workbook) -> Self {
        Engine { wb, memo: RefCell::new(HashMap::new()) }
    }

    /// What the cell shows once Excel has computed the file.
    pub fn value(&self, sheet: usize, row: u32, col: u32) -> Value {
        let s = &self.wb.sheets[sheet];
        let Some(formula) = s.formulas.get(&(row, col)) else {
            return s.value(row, col);
        };
        if let Some(v) = self.memo.borrow().get(&(sheet, row, col)) {
            return v.clone();
        }
        let v = self.eval(&parse(formula), sheet);
        self.memo.borrow_mut().insert((sheet, row, col), v.clone());
        v
    }

    fn sheet_of(&self, name: &Option<String>, here: usize) -> usize {
        match name {
            None => here,
            Some(n) => self.wb.index(n).unwrap_or_else(|| panic!("the formula refers to a sheet that is not in the file: {n}")),
        }
    }

    /// The cells of a range (row by row), or the one value of anything else.
    fn values(&self, e: &Expr, here: usize) -> Vec<Value> {
        match e {
            Expr::Range { sheet, from, to } => {
                let s = self.sheet_of(sheet, here);
                let mut out = Vec::new();
                for r in from.0..=to.0 {
                    for c in from.1..=to.1 {
                        out.push(self.value(s, r, c));
                    }
                }
                out
            }
            other => vec![self.eval(other, here)],
        }
    }

    fn numbers(&self, args: &[Expr], here: usize) -> Result<Vec<f64>, String> {
        let mut out = Vec::new();
        for a in args {
            if matches!(a, Expr::Range { .. }) {
                // In a range only the numbers count (text and empty cells are skipped).
                for v in self.values(a, here) {
                    match v {
                        Value::Num(n) => out.push(n),
                        Value::Error(e) => return Err(e),
                        _ => {}
                    }
                }
            } else {
                out.push(number(&self.eval(a, here))?);
            }
        }
        Ok(out)
    }

    fn eval(&self, e: &Expr, here: usize) -> Value {
        match self.try_eval(e, here) {
            Ok(v) => v,
            Err(err) => Value::Error(err),
        }
    }

    fn try_eval(&self, e: &Expr, here: usize) -> Result<Value, String> {
        Ok(match e {
            Expr::Num(n) => Value::Num(*n),
            Expr::Str(s) => Value::Text(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Cell { sheet, row, col } => {
                let v = self.value(self.sheet_of(sheet, here), *row, *col);
                if let Value::Error(err) = v {
                    return Err(err);
                }
                v
            }
            Expr::Range { .. } => return Err("VALUE (range used as a value)".into()),
            Expr::Neg(inner) => Value::Num(-number(&self.eval(inner, here))?),
            Expr::Bin(op, a, b) => {
                let (x, y) = (self.eval(a, here), self.eval(b, here));
                if let Value::Error(err) = &x {
                    return Err(err.clone());
                }
                if let Value::Error(err) = &y {
                    return Err(err.clone());
                }
                match op.as_str() {
                    "+" => Value::Num(number(&x)? + number(&y)?),
                    "-" => Value::Num(number(&x)? - number(&y)?),
                    "*" => Value::Num(number(&x)? * number(&y)?),
                    "/" => {
                        let d = number(&y)?;
                        if d == 0.0 {
                            return Err("DIV/0".into());
                        }
                        Value::Num(number(&x)? / d)
                    }
                    "^" => Value::Num(number(&x)?.powf(number(&y)?)),
                    "&" => Value::Text(format!("{}{}", text(&x), text(&y))),
                    cmp => {
                        let o = compare(&x, &y);
                        use std::cmp::Ordering::*;
                        Value::Bool(match cmp {
                            "=" => o == Equal,
                            "<>" => o != Equal,
                            "<" => o == Less,
                            ">" => o == Greater,
                            "<=" => o != Greater,
                            ">=" => o != Less,
                            _ => unreachable!(),
                        })
                    }
                }
            }
            Expr::Call(name, args) => self.call(name, args, here)?,
        })
    }

    fn call(&self, name: &str, args: &[Expr], here: usize) -> Result<Value, String> {
        let arg = |i: usize| self.eval(&args[i], here);
        Ok(match name {
            "SUM" => Value::Num(self.numbers(args, here)?.iter().sum()),
            "MAX" => Value::Num(self.numbers(args, here)?.into_iter().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))).unwrap_or(0.0)),
            "MIN" => Value::Num(self.numbers(args, here)?.into_iter().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x)))).unwrap_or(0.0)),
            "IF" => {
                let cond = arg(0);
                let yes = match &cond {
                    Value::Bool(b) => *b,
                    Value::Error(e) => return Err(e.clone()),
                    other => number(other)? != 0.0,
                };
                if yes {
                    arg(1)
                } else if args.len() > 2 {
                    arg(2)
                } else {
                    Value::Bool(false)
                }
            }
            "ROUND" => Value::Num(excel_round(number(&arg(0))?, number(&arg(1))? as i32)),
            "INT" => Value::Num(number(&arg(0))?.floor()),
            "MOD" => {
                let (a, b) = (number(&arg(0))?, number(&arg(1))?);
                if b == 0.0 {
                    return Err("DIV/0".into());
                }
                Value::Num(a - b * (a / b).floor())
            }
            "DATE" => {
                let (y, m, d) = (number(&arg(0))? as i32, number(&arg(1))? as u32, number(&arg(2))? as u32);
                Value::Num(serial(NaiveDate::from_ymd_opt(y, m, d).ok_or("NUM (date)")?))
            }
            "SUMIFS" | "SUMIF" | "COUNTIF" | "COUNTIFS" => {
                // SUMIF(range, criterion, [sum range]) · SUMIFS(sum range, range1, crit1, …) · COUNTIF(S)(range1, crit1, …)
                let (sum, pairs): (Option<&Expr>, Vec<(&Expr, &Expr)>) = match name {
                    "SUMIFS" => (Some(&args[0]), args[1..].chunks(2).map(|p| (&p[0], &p[1])).collect()),
                    "SUMIF" => (Some(args.get(2).unwrap_or(&args[0])), vec![(&args[0], &args[1])]),
                    _ => (None, args.chunks(2).map(|p| (&p[0], &p[1])).collect()),
                };
                let ranges: Vec<(Vec<Value>, Value)> = pairs.iter().map(|(r, c)| (self.values(r, here), self.eval(c, here))).collect();
                let len = ranges[0].0.len();
                assert!(ranges.iter().all(|(r, _)| r.len() == len), "{name}: ranges of different sizes");
                let sum_values = sum.map(|s| self.values(s, here));
                let mut total = 0.0;
                for i in 0..len {
                    if ranges.iter().all(|(r, c)| meets(&r[i], c)) {
                        total += match &sum_values {
                            Some(v) => v[i].num().unwrap_or(0.0),
                            None => 1.0,
                        };
                    }
                }
                Value::Num(total)
            }
            other => panic!("the test engine does not know the function {other}: add it to tests/support/formula.rs"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criteria_and_rounding_behave_like_excel() {
        assert!(meets(&Value::Text("Staționare".into()), &Value::Text("staționare".into())));
        assert!(meets(&Value::Empty, &Value::Text("<>Staționare".into())));
        assert!(!meets(&Value::Text("Completare".into()), &Value::Text("<>Completare".into())));
        assert!(meets(&Value::Num(46000.0), &Value::Num(46000.0)));
        assert_eq!(excel_round(2.5, 0), 3.0);
        assert_eq!(excel_round(0.3125 * 1440.0, 0), 450.0);
        assert!(is_cell("$H$6") && is_cell("AB12") && !is_cell("ROUND"));
        assert_eq!(cell_pos("$AB$12"), (11, 27));
    }
}
