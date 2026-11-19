//! The shape of a test input and the constraints on its values.
//!
//! A [`Schema`] says what lines an input has; [`Domains`] say what values go into them. Both have a
//! plain-text form, used for printing what was inferred from the samples, for the `format` and
//! `vars` hints, and for the answers typed at the `--ask-constraints` prompt.
//!
//! # The format
//!
//! One item per line, in the order the lines appear in the input:
//!
//! ```text
//! t                    a line of integers, each named (here: one, `t`)
//! repeat t {           the lines inside are repeated `t` times
//!   n m                two integers on one line
//!   a[n]               one line of `n` integers, named `a`
//!   str s[m]           one line holding a string of `m` characters
//!   repeat m {         `m` lines ...
//!     u v              ... each with two integers
//!   }
//! }
//! ```
//!
//! Lengths and counts are expressions over integers named earlier (`n`, `n-1`, `2*n`, `10^5`, `1e5`)
//! and `size`, the size of the test being generated. A name defined inside a `repeat` block can only
//! be used inside it. Every name is unique, so a constraint can refer to it unambiguously.
//!
//! # The constraints
//!
//! Each name has one constraint, written:
//!
//! ```text
//! 1..n                 an integer from 1 to n, both included (bounds are expressions)
//! 1..n, distinct       an array whose elements are all different
//! -5..5, sorted        an array in non-decreasing order
//! perm                 an array that is a permutation of 1..its length
//! chars a-z            a string over these characters (`chars 01`, `chars .#`)
//! ```
//!
//! A variable that sets a length or count, or that bounds another variable, is a *size variable*:
//! while a test of size `s` is generated it is limited to the first `s` values of its range, so
//! small tests come first however large the real limit is.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
};

/// The name of the variable that holds the size of the test being generated.
pub const SIZE: &str = "size";

// ---- expressions --------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    /// Integer division, rounding towards zero.
    Div,
    Pow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Num(i64),
    Var(String),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
}

impl Expr {
    pub fn num(n: i64) -> Expr {
        Expr::Num(n)
    }

    pub fn var(name: &str) -> Expr {
        Expr::Var(name.to_string())
    }

    pub fn bin(op: Op, left: Expr, right: Expr) -> Expr {
        Expr::Bin(op, Box::new(left), Box::new(right))
    }

    /// The variables the expression mentions, `size` included.
    pub fn vars(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.collect_vars(&mut out);
        out
    }

    fn collect_vars(&self, out: &mut BTreeSet<String>) {
        match self {
            Expr::Num(_) => {}
            Expr::Var(name) => {
                out.insert(name.clone());
            }
            Expr::Neg(inner) => inner.collect_vars(out),
            Expr::Bin(_, left, right) => {
                left.collect_vars(out);
                right.collect_vars(out);
            }
        }
    }

    /// Evaluates with checked arithmetic; `lookup` gives the value of a variable.
    ///
    /// This is a tiny interpreter for a parsed tree of integer `+ - * ^`: it never executes
    /// anything taken from the text, which only ever becomes numbers, names and those operators.
    pub fn eval(&self, lookup: &dyn Fn(&str) -> Option<i64>) -> Result<i64, String> {
        match self {
            Expr::Num(n) => Ok(*n),
            Expr::Var(name) => lookup(name).ok_or_else(|| format!("`{name}` has no value here")),
            Expr::Neg(inner) => inner.eval(lookup)?.checked_neg().ok_or_else(|| format!("`{self}` overflows")),
            Expr::Bin(op, left, right) => {
                let (l, r) = (left.eval(lookup)?, right.eval(lookup)?);
                let value = match op {
                    Op::Add => l.checked_add(r),
                    Op::Sub => l.checked_sub(r),
                    Op::Mul => l.checked_mul(r),
                    Op::Div => l.checked_div(r),
                    Op::Pow => u32::try_from(r).ok().and_then(|exp| l.checked_pow(exp)),
                };
                value.ok_or_else(|| format!("`{self}` overflows, divides by zero or has a negative exponent"))
            }
        }
    }

    pub fn parse(text: &str) -> Result<Expr, String> {
        let tokens = lex(text)?;
        if tokens.is_empty() {
            return Err("an expression is empty".to_string());
        }
        let mut parser = Parser { tokens, pos: 0 };
        let expr = parser.expr()?;
        match parser.tokens.get(parser.pos) {
            None => Ok(expr),
            Some(token) => Err(format!("unexpected `{token}` in `{}`", text.trim())),
        }
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, min: u8) -> fmt::Result {
        // Precedence: + - are 1, * / are 2, unary - is 3, ^ is 4, atoms are 5.
        match self {
            Expr::Num(n) if *n < 0 && min > 3 => write!(f, "({n})"),
            Expr::Num(n) => write!(f, "{n}"),
            Expr::Var(name) => write!(f, "{name}"),
            Expr::Neg(inner) => {
                if min > 3 {
                    write!(f, "(")?;
                }
                write!(f, "-")?;
                inner.write(f, 3)?;
                if min > 3 {
                    write!(f, ")")?;
                }
                Ok(())
            }
            Expr::Bin(op, left, right) => {
                let (prec, left_min, right_min, symbol) = match op {
                    Op::Add => (1, 1, 2, "+"),
                    Op::Sub => (1, 1, 2, "-"),
                    Op::Mul => (2, 2, 3, "*"),
                    Op::Div => (2, 2, 3, "/"),
                    Op::Pow => (4, 5, 3, "^"),
                };
                if min > prec {
                    write!(f, "(")?;
                }
                left.write(f, left_min)?;
                write!(f, "{symbol}")?;
                right.write(f, right_min)?;
                if min > prec {
                    write!(f, ")")?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, 0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Num(i64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Open,
    Close,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Num(n) => write!(f, "{n}"),
            Tok::Ident(name) => write!(f, "{name}"),
            Tok::Plus => write!(f, "+"),
            Tok::Minus => write!(f, "-"),
            Tok::Star => write!(f, "*"),
            Tok::Slash => write!(f, "/"),
            Tok::Caret => write!(f, "^"),
            Tok::Open => write!(f, "("),
            Tok::Close => write!(f, ")"),
        }
    }
}

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '+' => {
                tokens.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Tok::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Tok::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Tok::Slash);
                i += 1;
            }
            '^' => {
                tokens.push(Tok::Caret);
                i += 1;
            }
            '(' => {
                tokens.push(Tok::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Tok::Close);
                i += 1;
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    i += 1;
                }
                let digits: String = chars[start..i].iter().filter(|&&d| d != '_').collect();
                let mut value: i64 = digits.parse().map_err(|_| format!("the number `{digits}` is too large"))?;
                // `1e5` means 100000.
                if i + 1 < chars.len() && (chars[i] == 'e' || chars[i] == 'E') && chars[i + 1].is_ascii_digit() {
                    i += 1;
                    let exp_start = i;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let exp: String = chars[exp_start..i].iter().collect();
                    let exp: u32 = exp.parse().map_err(|_| format!("the exponent `{exp}` is too large"))?;
                    value = 10i64
                        .checked_pow(exp)
                        .and_then(|scale| value.checked_mul(scale))
                        .ok_or_else(|| format!("`{digits}e{exp}` is too large"))?;
                }
                if i < chars.len() && (chars[i].is_alphabetic() || chars[i] == '_') {
                    return Err(format!("write `{}*{}` instead of `{}{}`", value, chars[i], value, chars[i]));
                }
                tokens.push(Tok::Num(value));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(format!("unexpected character `{other}`")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    // expr = term (('+' | '-') term)*
    fn expr(&mut self) -> Result<Expr, String> {
        let mut left = self.term()?;
        while let Some(op) = match self.peek() {
            Some(Tok::Plus) => Some(Op::Add),
            Some(Tok::Minus) => Some(Op::Sub),
            _ => None,
        } {
            self.pos += 1;
            let right = self.term()?;
            left = Expr::bin(op, left, right);
        }
        Ok(left)
    }

    // term = unary (('*' | '/') unary)*
    fn term(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        while let Some(op) = match self.peek() {
            Some(Tok::Star) => Some(Op::Mul),
            Some(Tok::Slash) => Some(Op::Div),
            _ => None,
        } {
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::bin(op, left, right);
        }
        Ok(left)
    }

    // unary = '-' unary | power
    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Tok::Minus) {
            self.pos += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.power()
    }

    // power = atom ('^' unary)?
    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        if self.peek() == Some(&Tok::Caret) {
            self.pos += 1;
            let exponent = self.unary()?;
            return Ok(Expr::bin(Op::Pow, base, exponent));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.tokens.get(self.pos).cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Num(n))
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                Ok(Expr::Var(name))
            }
            Some(Tok::Open) => {
                self.pos += 1;
                let inner = self.expr()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err("a `(` is not closed".to_string());
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(other) => Err(format!("unexpected `{other}`")),
            None => Err("the expression ends too soon".to_string()),
        }
    }
}

// ---- values in scope -----------------------------------------------------------------

/// The values of the integer variables defined so far, in nested scopes (a `repeat` block opens one).
#[derive(Debug, Default)]
pub struct Env {
    frames: Vec<Vec<(String, i64)>>,
}

impl Env {
    /// An environment holding just `size`.
    pub fn with_size(size: i64) -> Env {
        Env { frames: vec![vec![(SIZE.to_string(), size)]] }
    }

    pub fn push(&mut self) {
        self.frames.push(Vec::new());
    }

    pub fn pop(&mut self) {
        self.frames.pop();
    }

    pub fn define(&mut self, name: &str, value: i64) {
        self.frames.last_mut().expect("an environment always has a frame").push((name.to_string(), value));
    }

    pub fn get(&self, name: &str) -> Option<i64> {
        self.frames.iter().rev().find_map(|frame| frame.iter().rev().find(|(n, _)| n == name).map(|(_, v)| *v))
    }

    /// The variables in scope, outermost first, without `size`.
    pub fn visible(&self) -> Vec<(String, i64)> {
        self.frames.iter().flatten().filter(|(n, _)| n != SIZE).cloned().collect()
    }

    pub fn eval(&self, expr: &Expr) -> Result<i64, String> {
        expr.eval(&|name| self.get(name))
    }
}

// ---- the format ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A line of integers, one per name.
    Scalars(Vec<String>),
    /// A line of `len` integers.
    Array { name: String, len: Expr },
    /// A line holding one string of `len` characters.
    Str { name: String, len: Expr },
    /// `count` repetitions of `body`.
    Repeat { count: Expr, body: Vec<Item> },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Schema {
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    Scalar,
    Array,
    Str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub kind: VarKind,
    /// The integer variables in scope where this one is defined, in definition order.
    pub visible: Vec<String>,
}

/// A list of edges in a format: how many rows, and the names of the two columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeBlock {
    pub columns: [String; 2],
    pub count: Expr,
}

impl EdgeBlock {
    /// The key of its constraint in [`Domains`]: the two names, e.g. `u v`.
    pub fn key(&self) -> String {
        format!("{} {}", self.columns[0], self.columns[1])
    }
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
}

impl Schema {
    /// Every variable in the order it appears in the input.
    pub fn variables(&self) -> Vec<Variable> {
        fn walk(items: &[Item], scope: &mut Vec<String>, out: &mut Vec<Variable>) {
            let base = scope.len();
            for item in items {
                match item {
                    Item::Scalars(names) => {
                        for name in names {
                            out.push(Variable { name: name.clone(), kind: VarKind::Scalar, visible: scope.clone() });
                            scope.push(name.clone());
                        }
                    }
                    Item::Array { name, .. } => {
                        out.push(Variable { name: name.clone(), kind: VarKind::Array, visible: scope.clone() });
                    }
                    Item::Str { name, .. } => {
                        out.push(Variable { name: name.clone(), kind: VarKind::Str, visible: scope.clone() });
                    }
                    Item::Repeat { body, .. } => walk(body, scope, out),
                }
            }
            scope.truncate(base);
        }
        let mut out = Vec::new();
        walk(&self.items, &mut Vec::new(), &mut out);
        out
    }

    /// The `repeat` blocks that are lists of pairs: a body that is one line of two numbers.
    pub fn edge_blocks(&self) -> Vec<EdgeBlock> {
        fn walk(items: &[Item], out: &mut Vec<EdgeBlock>) {
            for item in items {
                if let Item::Repeat { count, body } = item {
                    if let [Item::Scalars(names)] = body.as_slice()
                        && let [u, v] = names.as_slice()
                    {
                        out.push(EdgeBlock { columns: [u.clone(), v.clone()], count: count.clone() });
                    }
                    walk(body, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    /// The rows of the `repeat` blocks that are one line of two or more numbers, as column names.
    pub fn row_lines(&self) -> Vec<Vec<String>> {
        fn walk(items: &[Item], out: &mut Vec<Vec<String>>) {
            for item in items {
                if let Item::Repeat { body, .. } = item {
                    match body.as_slice() {
                        [Item::Scalars(names)] if names.len() >= 2 => out.push(names.clone()),
                        _ => walk(body, out),
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    /// The variables that appear in a length or a repeat count.
    pub fn length_vars(&self) -> BTreeSet<String> {
        fn walk(items: &[Item], out: &mut BTreeSet<String>) {
            for item in items {
                match item {
                    Item::Scalars(_) => {}
                    Item::Array { len, .. } | Item::Str { len, .. } => out.extend(len.vars()),
                    Item::Repeat { count, body } => {
                        out.extend(count.vars());
                        walk(body, out);
                    }
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(&self.items, &mut out);
        out.remove(SIZE);
        out
    }

    /// Checks that names are identifiers, unique, not `size`, and that every length and count
    /// only uses integers defined earlier in scope.
    pub fn validate(&self) -> Result<(), String> {
        fn check(items: &[Item], scope: &mut Vec<String>, seen: &mut HashSet<String>) -> Result<(), String> {
            let base = scope.len();
            fn define(name: &str, scope: &mut Vec<String>, seen: &mut HashSet<String>, is_int: bool) -> Result<(), String> {
                if !is_identifier(name) {
                    return Err(format!("`{name}` is not a valid name"));
                }
                if name == SIZE {
                    return Err(format!("`{SIZE}` is reserved for the size of the test"));
                }
                if !seen.insert(name.to_string()) {
                    return Err(format!("the name `{name}` is used twice; every name must be unique"));
                }
                if is_int {
                    scope.push(name.to_string());
                }
                Ok(())
            }
            let check_expr = |expr: &Expr, scope: &[String], what: &str| -> Result<(), String> {
                for var in expr.vars() {
                    if var != SIZE && !scope.contains(&var) {
                        return Err(format!("{what} uses `{var}`, which is not a number defined before it"));
                    }
                }
                Ok(())
            };
            for item in items {
                match item {
                    Item::Scalars(names) => {
                        if names.is_empty() {
                            return Err("a line of integers needs at least one name".to_string());
                        }
                        for name in names {
                            define(name, scope, seen, true)?;
                        }
                    }
                    Item::Array { name, len } => {
                        check_expr(len, scope, &format!("the length of `{name}`"))?;
                        define(name, scope, seen, false)?;
                    }
                    Item::Str { name, len } => {
                        check_expr(len, scope, &format!("the length of `{name}`"))?;
                        define(name, scope, seen, false)?;
                    }
                    Item::Repeat { count, body } => {
                        check_expr(count, scope, "a repeat count")?;
                        check(body, scope, seen)?;
                    }
                }
            }
            scope.truncate(base);
            Ok(())
        }
        if self.items.is_empty() {
            return Err("the format is empty".to_string());
        }
        check(&self.items, &mut Vec::new(), &mut HashSet::new())
    }

    /// The format as text, one item per line.
    pub fn lines(&self) -> Vec<String> {
        fn walk(items: &[Item], depth: usize, out: &mut Vec<String>) {
            let indent = "  ".repeat(depth);
            for item in items {
                match item {
                    Item::Scalars(names) => out.push(format!("{indent}{}", names.join(" "))),
                    Item::Array { name, len } => out.push(format!("{indent}{name}[{len}]")),
                    Item::Str { name, len } => out.push(format!("{indent}str {name}[{len}]")),
                    Item::Repeat { count, body } => {
                        out.push(format!("{indent}repeat {count} {{"));
                        walk(body, depth + 1, out);
                        out.push(format!("{indent}}}"));
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, 0, &mut out);
        out
    }

    /// Parses the text form (see the module documentation). `#` starts a comment.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Schema, String> {
        let cleaned: Vec<(usize, String)> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| (i + 1, line.as_ref().split('#').next().unwrap_or("").trim().to_string()))
            .filter(|(_, line)| !line.is_empty())
            .collect();

        fn block(lines: &[(usize, String)], pos: &mut usize, nested: bool) -> Result<Vec<Item>, String> {
            let mut items = Vec::new();
            while *pos < lines.len() {
                let (number, line) = &lines[*pos];
                *pos += 1;
                let at = |message: String| format!("format line {number} (`{line}`): {message}");

                if line == "}" {
                    if nested {
                        return Ok(items);
                    }
                    return Err(at("this `}` closes nothing".to_string()));
                }
                if let Some(rest) = line.strip_prefix("repeat ").or_else(|| line.strip_prefix("repeat\t")) {
                    let count = rest.trim().strip_suffix('{').ok_or_else(|| at("a `repeat` line must end with `{`".to_string()))?;
                    let count = Expr::parse(count).map_err(&at)?;
                    let body = block(lines, pos, true)?;
                    if body.is_empty() {
                        return Err(at("a `repeat` block is empty".to_string()));
                    }
                    items.push(Item::Repeat { count, body });
                } else if let Some(rest) = line.strip_prefix("str ") {
                    let (name, len) = bracketed(rest.trim()).ok_or_else(|| at("expected `str name[length]`".to_string()))?;
                    items.push(Item::Str { name: name.to_string(), len: Expr::parse(len).map_err(&at)? });
                } else if line.contains('[') {
                    let (name, len) = bracketed(line).ok_or_else(|| at("expected `name[length]`".to_string()))?;
                    items.push(Item::Array { name: name.to_string(), len: Expr::parse(len).map_err(&at)? });
                } else {
                    items.push(Item::Scalars(line.split_whitespace().map(str::to_string).collect()));
                }
            }
            if nested {
                return Err("a `repeat` block is not closed: a `}` is missing".to_string());
            }
            Ok(items)
        }

        /// `name[len]` -> (`name`, `len`)
        fn bracketed(text: &str) -> Option<(&str, &str)> {
            let open = text.find('[')?;
            let inner = text[open + 1..].strip_suffix(']')?;
            Some((text[..open].trim(), inner))
        }

        let schema = Schema { items: block(&cleaned, &mut 0, false)? };
        schema.validate()?;
        Ok(schema)
    }
}

// ---- the constraints --------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spec {
    /// An integer from `lo` to `hi`, both included. For an array the flags constrain its elements.
    Range { lo: Expr, hi: Expr, distinct: bool, sorted: bool },
    /// An array that is a permutation of `1..length`.
    Perm,
    /// A string over these characters (sorted, without repeats).
    Chars(Vec<char>),
    /// A list of edges: the rows of a block with one line of two numbers. Keyed by the two names,
    /// e.g. `u v`, not by one variable.
    Edges(EdgeSpec),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// The edges of a tree: one fewer than the vertices, connecting all of them.
    Tree,
    /// A simple graph: no loops, no repeated edges.
    Graph,
}

/// What a list of edges is: vertices are the numbers `lo..=hi` of the first column's constraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeSpec {
    pub kind: EdgeKind,
    /// An edge `u v` differs from `v u` (a graph only).
    pub directed: bool,
    /// All vertices are connected (always true of a tree).
    pub connected: bool,
    /// Every edge is written with `u < v` (not with `directed`).
    pub ordered: bool,
}

pub type Domains = BTreeMap<String, Spec>;

impl Spec {
    pub fn range(lo: Expr, hi: Expr) -> Spec {
        Spec::Range { lo, hi, distinct: false, sorted: false }
    }

    /// The variables the constraint mentions, `size` included.
    pub fn vars(&self) -> BTreeSet<String> {
        match self {
            Spec::Range { lo, hi, .. } => lo.vars().union(&hi.vars()).cloned().collect(),
            Spec::Perm | Spec::Chars(_) | Spec::Edges(_) => BTreeSet::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Spec, String> {
        let text = text.trim();
        if text == "perm" {
            return Ok(Spec::Perm);
        }
        if matches!(text.split(',').next().map(str::trim), Some("tree" | "graph")) {
            return parse_edges(text);
        }
        if let Some(rest) = text.strip_prefix("chars")
            && (rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            return parse_alphabet(rest.trim());
        }

        let mut parts = text.split(',');
        let range = parts.next().unwrap_or("");
        let (lo, hi) = range
            .split_once("..")
            .ok_or_else(|| format!("expected `lo..hi`, `perm` or `chars ...`, got `{text}`"))?;
        let (mut distinct, mut sorted) = (false, false);
        for flag in parts {
            match flag.trim() {
                "distinct" => distinct = true,
                "sorted" => sorted = true,
                other => return Err(format!("unknown flag `{other}` (the flags are `distinct` and `sorted`)")),
            }
        }
        Ok(Spec::Range {
            lo: Expr::parse(lo).map_err(|e| format!("in the lower bound: {e}"))?,
            hi: Expr::parse(hi).map_err(|e| format!("in the upper bound: {e}"))?,
            distinct,
            sorted,
        })
    }
}

fn parse_edges(text: &str) -> Result<Spec, String> {
    let mut parts = text.split(',').map(str::trim);
    let kind = if parts.next() == Some("tree") { EdgeKind::Tree } else { EdgeKind::Graph };
    let mut spec = EdgeSpec { kind, directed: false, connected: kind == EdgeKind::Tree, ordered: false };
    for flag in parts {
        match flag {
            "directed" => spec.directed = true,
            "connected" => spec.connected = true,
            "ordered" => spec.ordered = true,
            other => return Err(format!("unknown flag `{other}` for edges (the flags are `connected`, `directed` and `ordered`)")),
        }
    }
    if spec.kind == EdgeKind::Tree && spec.directed {
        return Err("a tree cannot be `directed`; use `graph, directed`".to_string());
    }
    if spec.directed && spec.ordered {
        return Err("`directed` and `ordered` contradict each other: an ordered edge always goes from the smaller to the larger".to_string());
    }
    Ok(Spec::Edges(spec))
}

fn parse_alphabet(text: &str) -> Result<Spec, String> {
    if text.is_empty() {
        return Err("`chars` needs the characters, e.g. `chars a-z` or `chars 01`".to_string());
    }
    if text.chars().any(char::is_whitespace) {
        return Err("the characters of `chars` cannot include spaces".to_string());
    }
    let chars: Vec<char> = text.chars().collect();
    let mut set = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        // `x-y` is a range when something comes before and after the dash.
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let (from, to) = (chars[i], chars[i + 2]);
            if from > to {
                return Err(format!("the range `{from}-{to}` is backwards"));
            }
            set.extend((from..=to).filter(|c| !c.is_control()));
            i += 3;
        } else {
            set.insert(chars[i]);
            i += 1;
        }
    }
    Ok(Spec::Chars(set.into_iter().collect()))
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Spec::Range { lo, hi, distinct, sorted } => {
                write!(f, "{lo}..{hi}")?;
                if *distinct {
                    write!(f, ", distinct")?;
                }
                if *sorted {
                    write!(f, ", sorted")?;
                }
                Ok(())
            }
            Spec::Perm => write!(f, "perm"),
            Spec::Edges(edges) => {
                write!(f, "{}", if edges.kind == EdgeKind::Tree { "tree" } else { "graph" })?;
                if edges.kind == EdgeKind::Graph && edges.connected {
                    write!(f, ", connected")?;
                }
                if edges.directed {
                    write!(f, ", directed")?;
                }
                if edges.ordered {
                    write!(f, ", ordered")?;
                }
                Ok(())
            }
            Spec::Chars(chars) => {
                // A dash must come first or last to be read as a character, not as a range.
                let mut text = String::new();
                if chars.contains(&'-') {
                    text.push('-');
                }
                let rest: Vec<char> = chars.iter().copied().filter(|&c| c != '-').collect();
                let mut i = 0;
                while i < rest.len() {
                    let mut j = i;
                    while j + 1 < rest.len() && rest[j + 1] as u32 == rest[j] as u32 + 1 {
                        j += 1;
                    }
                    if j - i >= 2 {
                        text.push(rest[i]);
                        text.push('-');
                        text.push(rest[j]);
                    } else {
                        text.extend(&rest[i..=j]);
                    }
                    i = j + 1;
                }
                write!(f, "chars {text}")
            }
        }
    }
}

/// Checks that every variable has a constraint of a kind that suits it, that constraints only use
/// numbers defined before the variable (or `size`), and that nothing is constrained that does not exist.
pub fn validate_domains(schema: &Schema, domains: &Domains) -> Result<(), String> {
    let variables = schema.variables();
    for variable in &variables {
        let spec = domains.get(&variable.name).ok_or_else(|| format!("`{}` has no constraint", variable.name))?;
        validate_spec(variable, spec)?;
    }
    let blocks: HashSet<String> = schema.edge_blocks().iter().map(EdgeBlock::key).collect();
    let known: HashSet<&str> = variables.iter().map(|v| v.name.as_str()).collect();
    for (key, spec) in domains {
        if key.contains(' ') {
            // The constraint of a list of edges, keyed by its two columns.
            if !blocks.contains(key) {
                let lists: Vec<String> = blocks.iter().cloned().collect();
                return Err(format!(
                    "`{key}` is not a list of edges in the input format (a repeated block holding one line of two numbers){}",
                    if lists.is_empty() { String::new() } else { format!("; the lists are: {}", lists.join(", ")) }
                ));
            }
            if !matches!(spec, Spec::Edges(_)) {
                return Err(format!("`{key}` is a list of edges; its constraint is `tree` or `graph` (with flags), not `{spec}`"));
            }
        } else if !known.contains(key.as_str()) {
            let names: Vec<&str> = variables.iter().map(|v| v.name.as_str()).collect();
            return Err(format!("`{key}` is not a variable of the input format (the variables are: {})", names.join(", ")));
        }
    }
    Ok(())
}

/// Checks one constraint against the variable it is for.
pub fn validate_spec(variable: &Variable, spec: &Spec) -> Result<(), String> {
    let name = &variable.name;
    match (variable.kind, spec) {
        (_, Spec::Edges(_)) => {
            return Err(format!(
                "`{name}` is one variable; `tree` and `graph` are constraints of a pair of columns that list edges, written with both names, e.g. `u v`"
            ));
        }
        (VarKind::Scalar, Spec::Range { distinct: false, sorted: false, .. }) => {}
        (VarKind::Scalar, Spec::Range { .. }) => {
            return Err(format!("`{name}` is a single number: `distinct` and `sorted` are for arrays"));
        }
        (VarKind::Scalar, Spec::Perm) => return Err(format!("`{name}` is a single number, not an array: `perm` does not apply")),
        (VarKind::Array, Spec::Range { .. } | Spec::Perm) => {}
        (VarKind::Str, Spec::Chars(_)) => {}
        (VarKind::Str, _) => return Err(format!("`{name}` is a string; its constraint is `chars ...`, e.g. `chars a-z`")),
        (_, Spec::Chars(_)) => return Err(format!("`{name}` holds numbers; `chars ...` is for strings")),
    }
    for var in spec.vars() {
        if var != SIZE && !variable.visible.contains(&var) {
            return Err(format!("the constraint of `{name}` uses `{var}`, which is not a number defined before it"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Expr {
        Expr::parse(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
    }

    fn eval(text: &str, vars: &[(&str, i64)]) -> Result<i64, String> {
        parse(text).eval(&|name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| *v))
    }

    // ---- expressions ---------------------------------------------------------------

    #[test]
    fn expressions_follow_the_usual_precedence() {
        assert_eq!(eval("1+2*3", &[]), Ok(7));
        assert_eq!(eval("(1+2)*3", &[]), Ok(9));
        assert_eq!(eval("10-3-2", &[]), Ok(5), "subtraction is left-associative");
        assert_eq!(eval("2^3^2", &[]), Ok(512), "powers are right-associative");
        assert_eq!(eval("-2^2", &[]), Ok(-4));
        assert_eq!(eval("(-2)^2", &[]), Ok(4));
        assert_eq!(eval("2*-3", &[]), Ok(-6));
    }

    #[test]
    fn division_rounds_towards_zero_and_has_the_precedence_of_multiplication() {
        assert_eq!(eval("7/2", &[]), Ok(3));
        assert_eq!(eval("-7/2", &[]), Ok(-3));
        assert_eq!(eval("n*(n-1)/2", &[("n", 5)]), Ok(10), "the number of pairs of 5 things");
        assert_eq!(eval("12/2/3", &[]), Ok(2), "left-associative");
        assert_eq!(eval("2+6/3", &[]), Ok(4));
        assert!(eval("1/0", &[]).unwrap_err().contains("divides by zero"));
        for text in ["n*(n-1)/2", "n/(m/2)", "n/m/2", "(n+1)/2"] {
            let expr = parse(text);
            assert_eq!(Expr::parse(&expr.to_string()), Ok(expr.clone()), "{text}");
        }
    }

    #[test]
    fn expressions_use_variables_and_the_usual_competitive_notations() {
        assert_eq!(eval("n-1", &[("n", 5)]), Ok(4));
        assert_eq!(eval("2*n+m", &[("n", 5), ("m", 3)]), Ok(13));
        assert_eq!(eval("10^9", &[]), Ok(1_000_000_000));
        assert_eq!(eval("1e5", &[]), Ok(100_000));
        assert_eq!(eval("2e5+1", &[]), Ok(200_001));
        assert_eq!(eval("1_000", &[]), Ok(1000));
        assert_eq!(eval("size", &[("size", 7)]), Ok(7));
    }

    #[test]
    fn evaluation_reports_what_goes_wrong() {
        assert!(eval("n", &[]).unwrap_err().contains("`n` has no value"));
        assert!(eval("10^30", &[]).unwrap_err().contains("overflows"));
        assert!(eval("2^-1", &[]).unwrap_err().contains("negative exponent"));
        assert!(eval("9223372036854775807+1", &[]).unwrap_err().contains("overflows"));
    }

    #[test]
    fn bad_expressions_are_errors_with_a_hint() {
        assert!(Expr::parse("").is_err());
        assert!(Expr::parse("1+").unwrap_err().contains("ends too soon"));
        assert!(Expr::parse("(1+2").unwrap_err().contains("not closed"));
        assert!(Expr::parse("1 2").unwrap_err().contains("unexpected `2`"));
        assert!(Expr::parse("2n").unwrap_err().contains("2*n"));
        assert!(Expr::parse("1%2").unwrap_err().contains("unexpected character `%`"));
        assert!(Expr::parse("99999999999999999999").unwrap_err().contains("too large"));
    }

    #[test]
    fn expressions_print_so_that_they_parse_back_to_the_same_tree() {
        for text in [
            "1", "n", "n-1", "n-(m-1)", "n-m-1", "n+(m+1)", "2*(n+1)", "2*n*m", "2*(n*m)", "-n", "-(n+1)", "-n+1", "2^n", "(2^n)^m",
            "2^n^m", "(-2)^n", "-2^n", "n*-1", "10^9+7", "size*2", "(n+m)*(n-m)",
        ] {
            let expr = parse(text);
            let printed = expr.to_string();
            assert_eq!(Expr::parse(&printed), Ok(expr.clone()), "`{text}` printed as `{printed}`");
        }
        assert_eq!(parse("n - 1").to_string(), "n-1");
        assert_eq!(parse("n-(m-1)").to_string(), "n-(m-1)");
    }

    #[test]
    fn vars_lists_every_variable_once() {
        let vars: Vec<String> = parse("n*n + m - size").vars().into_iter().collect();
        assert_eq!(vars, ["m", "n", "size"]);
    }

    #[test]
    fn env_scopes_shadow_and_forget() {
        let mut env = Env::with_size(9);
        env.define("n", 3);
        env.push();
        env.define("n", 4);
        env.define("x", 1);
        assert_eq!(env.get("n"), Some(4), "the inner definition wins");
        assert_eq!(env.get("size"), Some(9));
        assert_eq!(env.visible(), [("n".to_string(), 3), ("n".to_string(), 4), ("x".to_string(), 1)]);
        env.pop();
        assert_eq!(env.get("n"), Some(3));
        assert_eq!(env.get("x"), None);
    }

    // ---- the format --------------------------------------------------------------------

    fn schema(text: &str) -> Schema {
        let lines: Vec<&str> = text.lines().collect();
        Schema::parse(&lines).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    #[test]
    fn a_format_prints_and_parses_back_to_itself() {
        let text = "t\nrepeat t {\n  n m\n  a[n]\n  str s[m]\n  repeat m {\n    u v\n  }\n}";
        let parsed = schema(text);
        assert_eq!(parsed.lines().join("\n"), text);
        assert_eq!(Schema::parse(&parsed.lines()), Ok(parsed));
    }

    #[test]
    fn the_format_accepts_comments_blank_lines_and_extra_spaces() {
        let parsed = schema("  n   m   # sizes\n\n a[ n-1 ]\n");
        assert_eq!(parsed.lines(), ["n m", "a[n-1]"]);
    }

    #[test]
    fn variables_come_in_input_order_with_what_is_visible_to_them() {
        let vars = schema("t\nrepeat t {\n n\n a[n]\n}").variables();
        let summary: Vec<(String, VarKind, Vec<String>)> = vars.into_iter().map(|v| (v.name, v.kind, v.visible)).collect();
        assert_eq!(
            summary,
            [
                ("t".to_string(), VarKind::Scalar, vec![]),
                ("n".to_string(), VarKind::Scalar, vec!["t".to_string()]),
                ("a".to_string(), VarKind::Array, vec!["t".to_string(), "n".to_string()]),
            ]
        );
    }

    #[test]
    fn variables_defined_in_a_repeat_block_are_not_visible_after_it() {
        let vars = schema("n\nrepeat n {\n u v\n}\nk").variables();
        assert_eq!(vars.last().unwrap().visible, ["n"], "`u` and `v` are local to the block");
    }

    #[test]
    fn length_vars_are_the_numbers_lengths_and_counts_use() {
        let lengths: Vec<String> = schema("n m k\na[n]\nrepeat m {\n u v\n}").length_vars().into_iter().collect();
        assert_eq!(lengths, ["m", "n"]);
    }

    fn format_error(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        Schema::parse(&lines).unwrap_err()
    }

    #[test]
    fn a_bad_format_is_explained() {
        assert!(format_error("").contains("empty"));
        assert!(format_error("n\na[m]").contains("`m`, which is not a number defined before it"));
        assert!(format_error("n\nn").contains("used twice"));
        assert!(format_error("size").contains("reserved"));
        assert!(format_error("a b\na[b]\nb").contains("used twice"));
        assert!(format_error("n\nrepeat n {\n x\n").contains("not closed"));
        assert!(format_error("n\n}").contains("closes nothing"));
        assert!(format_error("n\nrepeat n\n x\n}").contains("must end with `{`"));
        assert!(format_error("n\nrepeat n {\n}").contains("empty"));
        assert!(format_error("n\na[n").contains("expected `name[length]`"));
        assert!(format_error("1n").contains("not a valid name"));
        // An array's name is not a number a later length can use.
        assert!(format_error("n\na[n]\nb[a]").contains("`a`"));
        // A name from inside a block cannot be used after it.
        assert!(format_error("n\nrepeat n {\n x\n}\na[x]").contains("`x`"));
    }

    // ---- the constraints ---------------------------------------------------------------

    fn spec(text: &str) -> Spec {
        Spec::parse(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
    }

    #[test]
    fn constraints_print_and_parse_back_to_themselves() {
        for text in ["1..n", "0..10^9", "-5..5", "1..n, distinct", "1..n, sorted", "1..2*size, distinct, sorted", "perm", "chars a-z", "chars 01", "chars .#"] {
            let parsed = spec(text);
            assert_eq!(Spec::parse(&parsed.to_string()), Ok(parsed.clone()), "{text} -> {parsed}");
        }
        assert_eq!(spec("1 .. n , distinct").to_string(), "1..n, distinct");
        assert_eq!(spec("1..n,sorted,distinct").to_string(), "1..n, distinct, sorted");
    }

    #[test]
    fn character_sets_are_sorted_deduplicated_and_compressed() {
        assert_eq!(spec("chars cbaab"), Spec::Chars(vec!['a', 'b', 'c']));
        assert_eq!(spec("chars cbaab").to_string(), "chars a-c");
        assert_eq!(spec("chars ab").to_string(), "chars ab", "two neighbours are not worth a range");
        assert_eq!(spec("chars a-zA-Z0-9").to_string(), "chars 0-9A-Za-z");
        assert_eq!(spec("chars .#").to_string(), "chars #.");
    }

    #[test]
    fn a_dash_is_a_character_when_it_cannot_be_a_range() {
        assert_eq!(spec("chars -a"), Spec::Chars(vec!['-', 'a']));
        assert_eq!(spec("chars a-"), Spec::Chars(vec!['-', 'a']));
        assert_eq!(spec("chars +-"), Spec::Chars(vec!['+', '-']));
        // Printed with the dash first, so that it is not read as a range again.
        let spec = Spec::Chars(vec!['*', '-', '0']);
        assert_eq!(Spec::parse(&spec.to_string()), Ok(spec));
    }

    #[test]
    fn a_list_of_edges_has_a_constraint_that_prints_and_parses_back() {
        for text in ["tree", "tree, ordered", "graph", "graph, connected", "graph, directed", "graph, ordered", "graph, connected, ordered", "graph, connected, directed"] {
            let parsed = spec(text);
            assert_eq!(Spec::parse(&parsed.to_string()), Ok(parsed.clone()), "{text} -> {parsed}");
        }
        assert_eq!(spec("tree, connected").to_string(), "tree", "a tree is always connected");
        assert_eq!(spec("graph,ordered,connected").to_string(), "graph, connected, ordered");
        let error = |text: &str| Spec::parse(text).unwrap_err();
        assert!(error("tree, directed").contains("cannot be `directed`"));
        assert!(error("graph, directed, ordered").contains("contradict"));
        assert!(error("graph, dense").contains("unknown flag `dense`"));
    }

    #[test]
    fn the_lists_of_edges_in_a_format_are_the_repeats_of_one_line_of_two_numbers() {
        let s = schema("n m\nrepeat m {\n u v\n}\nrepeat n {\n a b c\n}\nt\nrepeat t {\n k\n x y\n}");
        let keys: Vec<String> = s.edge_blocks().iter().map(EdgeBlock::key).collect();
        assert_eq!(keys, ["u v"]);
        assert_eq!(s.row_lines(), [vec!["u", "v"], vec!["a", "b", "c"]], "the body of the last repeat has two lines, so it is not rows");
    }

    #[test]
    fn a_constraint_on_a_list_of_edges_is_checked_against_the_format() {
        let s = schema("n m\nrepeat m {\n u v\n}\na[n]");
        let base: Vec<(&str, &str)> = vec![("n", "1..size"), ("m", "0..n*(n-1)/2"), ("u", "1..n"), ("v", "1..n"), ("a", "1..9")];
        let with = |extra: &[(&str, &str)]| {
            let domains: Domains = base.iter().chain(extra).map(|(n, t)| (n.to_string(), spec(t))).collect();
            validate_domains(&s, &domains)
        };
        assert_eq!(with(&[("u v", "graph")]), Ok(()));
        assert!(with(&[("n m", "graph")]).unwrap_err().contains("`n m` is not a list of edges"));
        assert!(with(&[("u v", "1..5")]).unwrap_err().contains("its constraint is `tree` or `graph`"));
        assert!(with(&[("a", "tree")]).unwrap_err().contains("is one variable"));
    }

    #[test]
    fn bad_constraints_are_explained() {
        let error = |text: &str| Spec::parse(text).unwrap_err();
        assert!(error("5").contains("expected `lo..hi`"));
        assert!(error("1..").contains("upper bound"));
        assert!(error("..5").contains("lower bound"));
        assert!(error("1..5, unique").contains("unknown flag `unique`"));
        assert!(error("chars").contains("needs the characters"));
        assert!(error("chars z-a").contains("backwards"));
        assert!(error("chars a b").contains("spaces"));
    }

    #[test]
    fn constraints_are_validated_against_the_variable_they_are_for() {
        let s = schema("n\na[n]\nstr w[n]");
        let mut domains: Domains = [
            ("n", "1..size"),
            ("a", "1..n, distinct"),
            ("w", "chars a-z"),
        ]
        .into_iter()
        .map(|(name, text)| (name.to_string(), spec(text)))
        .collect();
        assert_eq!(validate_domains(&s, &domains), Ok(()));

        let with = |name: &str, text: &str| {
            let mut changed = domains.clone();
            changed.insert(name.to_string(), spec(text));
            validate_domains(&s, &changed)
        };
        assert!(with("n", "1..5, distinct").unwrap_err().contains("single number"));
        assert!(with("n", "perm").unwrap_err().contains("single number"));
        assert!(with("a", "chars a-z").unwrap_err().contains("holds numbers"));
        assert!(with("w", "1..5").unwrap_err().contains("is a string"));
        assert!(with("a", "1..m").unwrap_err().contains("`m`"));
        assert!(with("n", "1..n").unwrap_err().contains("`n`"), "a number cannot depend on itself");
        assert!(with("zzz", "1..5").unwrap_err().contains("not a variable"));

        domains.remove("w");
        assert!(validate_domains(&s, &domains).unwrap_err().contains("`w` has no constraint"));
    }
}
