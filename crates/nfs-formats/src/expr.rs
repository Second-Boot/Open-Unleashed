//! The front end's **expression evaluator** — every `.ctr` / `.lay` value that is arithmetic goes
//! through it. Read to the instruction on 2026-09-26 (`RE/notes/layouts.md` §"The expression
//! evaluator"); every rule below carries the address that decides it.
//!
//! The original runs a value through two passes, `0x52DC40(text)`:
//!
//! 1. **macro expansion** over the dictionary `[0x69CAF8]` (`0x52E5C0` → `0x52E1C0`): every
//!    identifier `[A-Za-z_.][A-Za-z0-9_.]*` that names a macro is replaced by that macro's
//!    **text**, expanded again, so `data.width` is spliced in as whatever string the `.lay` wrote
//!    (`260+95` stays `260+95`, it is not a number yet). A macro met again while it is being
//!    expanded fails the whole expansion, which leaves an empty string (0x52E258, 0x52E62A). A
//!    non-identifier run that is exactly `##` is dropped (0x52E305, the string at 0x5DDC60);
//! 2. **evaluation** by the object `[0x69CAFC]`: `0x52DF40` copies the text into a 0x200-byte
//!    buffer, turns every `.` into `X` (0x52DF6E…0x52DF73), and hands it to `0x52F830`.
//!
//! `0x52F830` is a two-stack operator-precedence parser over **32-bit integers**, lifted from an
//! assembler (its operand reader knows `$`/`%`/`@`/`&` radix prefixes, `h`/`d`/`b`/`o`/`q`
//! suffixes, character constants, a location counter `*`, `^X` control characters and `narg`).
//! Its two tables are built by `0x52F540`: `prec[c]` at `ctx+0` and `unary[c]` at `ctx+0x100`.
//!
//! An identifier that survived expansion is looked up in the evaluator's own symbol table
//! (`0x531470`), which nothing in the game fills (its only writer `0x531750` is reached for a
//! token ending in `#`, and `#` always ends a token), so **an undefined name reads 0**
//! (`0x530F0A`…`0x530F4D`) and the expression carries on. The error reporter `0x531C30` only
//! prints; the one error that changes a result is a syntax error in `0x52F830` itself, which
//! returns **0 for the whole expression** (0x52FE6C…0x52FE95).
//!
//! **The port's one addition** (APPROXIMATION, labelled, kept OPEN 2026-09-30): a name
//! the *port* cannot answer is carried as an unknown value instead of a number, so a caller can
//! keep its own fallback. The original has no such state (it always measures the real string);
//! for every name the port does answer, the result is the original's. Measured over all 98 shipped
//! `.lay` files (`--layout-sweep`, 59 461 evaluations): no part expression of the sweep fails
//! (`expr_failures` = 0). Until 2026-09-30 54 evaluations ended unknown — `box_template`
//! with `measure = false` (`BottomBarButton.ctr`'s `size.value`, `Graphic Tab Button.ctr`'s and
//! `Label.ctr`'s, the box pass that did not take a text/shape part's extent); that pass now
//! measures like the original (`layout.rs` `reference`), so the only remaining unknowns are a
//! name the port has no answer for and the depth guard of a cycle no shipped template has.

/// What a name in an expression stands for — the answer of the macro dictionary `[0x69CAF8]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Macro {
    /// A defined macro: its **text**, spliced in and expanded again (0x52E28E…0x52E2A9).
    Text(String),
    /// No such macro: the name stays as written and the evaluator reads it as **0** (0x530F0A).
    Undefined,
    /// The port cannot answer this name. Not a state of the original — see the module note.
    Unanswerable,
}

/// What stands in the expanded text for an [`Macro::Unanswerable`] name. It starts with a
/// backquote, a byte no shipped expression contains, so it cannot collide with a real name.
pub const UNKNOWN: &str = "`unanswerable`";

/// The sentinel `?` leaves when its condition is false and `:` looks for (0x530831 / 0x530842).
const NOT_TAKEN: i32 = 0x7FFF_FFFB;

/// `0x52E540`: a byte that can start an identifier.
fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'.'
}

/// `0x52E570`: a byte that can continue one (`0x52E540` or a digit, `0x52E5A0`).
fn ident_cont(c: u8) -> bool {
    ident_start(c) || c.is_ascii_digit()
}

/// `0x52E1C0` — expand every macro in `text`. `None` is the failed expansion (a macro met again
/// while it is being expanded, 0x52E258), which `0x52E5C0` turns into an empty string.
fn expand_into(text: &str, lookup: &dyn Fn(&str) -> Macro, active: &mut Vec<String>) -> Option<String> {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        // 0x52E380 with flag 0: an identifier run
        if ident_start(b[i]) {
            let s = i;
            i += 1;
            while i < b.len() && ident_cont(b[i]) {
                i += 1;
            }
            let name = &text[s..i];
            match lookup(name) {
                Macro::Text(t) => {
                    // the node's `[+0x1C]` guard, cleared for every node by 0x52E5DC
                    if active.iter().any(|a| a == name) {
                        return None;
                    }
                    active.push(name.to_string());
                    let x = expand_into(&t, lookup, active)?;
                    active.pop();
                    out.push_str(&x);
                }
                Macro::Undefined => out.push_str(name),
                Macro::Unanswerable => out.push_str(UNKNOWN),
            }
        } else {
            // 0x52E380 with flag 1: a run that does not start an identifier, up to the next byte
            // that could continue one — so digits come one at a time, which changes nothing
            let s = i;
            i += 1;
            while i < b.len() && !ident_cont(b[i]) {
                i += 1;
            }
            let run = &text[s..i];
            if run != "##" {
                out.push_str(run);
            }
        }
    }
    Some(out)
}

/// `0x52E5C0` — the whole first pass: every macro in `text` replaced by its expanded text, or
/// the empty string if the expansion failed.
pub fn expand(text: &str, lookup: &dyn Fn(&str) -> Macro) -> String {
    expand_into(text, lookup, &mut Vec::new()).unwrap_or_default()
}

/// `0x52DC40` — expand, then evaluate. `None` only when a name the port cannot answer decides the
/// result (the module note); the original always yields a number.
pub fn eval(text: &str, lookup: &dyn Fn(&str) -> Macro) -> Option<i32> {
    evaluate(&expand(text, lookup))
}

/// `0x52DF40` + `0x52F830` — evaluate text that has already been expanded.
pub fn evaluate(text: &str) -> Option<i32> {
    // the 0x200-byte buffer of 0x52DF4A: `strncpy` with 0x200, so a longer value has no
    // terminator in the original (0x52DF4A `lea edx,[esp]`, 0x52DF53 `push 0x200`, 0x52DF5A
    // `call 0x5A28A0` = `strncpy`, a 0x200-byte frame `sub esp,0x200` at 0x52DF44) and the scan of
    // 0x52DF6E…0x52DF7C then runs on into the stack. UNREACHABLE (2026-09-30): cut at
    // 0x1FF here. No shipped text is that long — 48 428 `key=value` lines of the 98 `.lay` and 75
    // `.ctr` files have a longest value of 407 bytes (a list of attribute names, never evaluated),
    // and of the 59 461 expanded texts the evaluator got over all 98 layouts (`--layout-sweep`)
    // the longest is 44 bytes.
    let mut s: Vec<u8> = text.bytes().take(0x1FF).map(|c| if c == b'.' { b'X' } else { c }).collect();
    s.extend_from_slice(&[0; 4]);
    let v = Evaluator::new().run(&s);
    v.known.then_some(v.v)
}

/// `0x527B80`, a point attribute's evaluation: the value is cut at its **first** comma (`strchr`,
/// 0x527BA9) and each side goes through `0x52DC40`, the rest of the string being the second half
/// whatever it holds. A value with **no comma** is not evaluated at all (`je 0x527CAD`), so the
/// attribute keeps what it had — `None` here, and the caller keeps its own value.
pub fn eval_pair(text: &str, lookup: &dyn Fn(&str) -> Macro) -> Option<(i32, i32)> {
    let text: String = text.chars().take(0x1FF).collect();
    let (a, b) = text.split_once(',')?;
    Some((eval(a, lookup)?, eval(b, lookup)?))
}

/// `prec[c]`, the binary-operator table `0x52F540` builds at `ctx+0`. The two-byte operators get
/// codes above 0x7F: `<<` 0x80, `>>` 0x81, `<=` 0x82, `>=` 0x83, `==` 0x84, `!=`/`<>` 0x85,
/// `&&`/`**` 0x86, `||`/`++` 0x87, `--` 0x88, `!:` 0x89, `!?` 0x8A; 0x8B is the implicit `[`.
fn prec(c: u8) -> i32 {
    match c {
        0 | b'(' | b'[' | b',' | b';' => 1,
        b')' | b']' => 2,
        b'?' | b':' | 0x89 | 0x8A => 3,
        0x87 | 0x88 => 4,
        0x86 => 5,
        b'|' | b'!' => 6,
        b'^' | b'~' => 7,
        b'&' => 8,
        b'=' | b'#' | 0x84 | 0x85 => 9,
        b'<' | b'>' | 0x82 | 0x83 => 10,
        0x80 | 0x81 => 11,
        b'+' | b'-' => 12,
        b'*' | b'/' | b'%' => 13,
        b'.' | 0x8B => 15,
        _ => 0,
    }
}

/// `unary[c]`, the table at `ctx+0x100`: 14 for the unary operators, 15 for the functions — except
/// `intcos` (0xA4), which 0x52F5EA…0x52F650 skips, so it is pushed with priority 0 — and 1/2 for
/// the brackets and separators.
fn unary(c: u8) -> i32 {
    match c {
        0x93..=0xA3 | 0xA5 => 15,
        0x8C..=0x92 | b'*' | b'+' | b'-' | b'<' | b'>' | b'~' | b'!' => 14,
        b')' | b']' => 2,
        0 | b'(' | b'[' | b',' | b';' => 1,
        _ => 0,
    }
}

/// The function names, in the order of the pointer table at 0x5DDE64; name *k* is operator
/// `0x93 + k` (0x52FB4D).
const FUNCTIONS: [&str; 19] = [
    "abs", "isalpha", "isupper", "islower", "isdigit", "isxdigit", "isalnum", "isspace", "ispunct", "iscntrl", "isprint",
    "isgraph", "isascii", "tolower", "toupper", "toascii", "intsin", "intcos", "sqrt",
];

/// The C runtime's `_pctype` in the "C" locale (the `is*` functions read it directly while
/// `__mb_cur_max` is 1, 0x530128): `_UPPER` 1, `_LOWER` 2, `_DIGIT` 4, `_SPACE` 8, `_PUNCT` 0x10,
/// `_CONTROL` 0x20, `_BLANK` 0x40, `_HEX` 0x80, `_ALPHA` 0x100; nothing above 0x7F.
fn ctype(c: i32) -> i32 {
    match c as u8 {
        b'\t'..=b'\r' => 0x28,
        0..=0x1F | 0x7F => 0x20,
        b' ' => 0x48,
        b'0'..=b'9' => 0x84,
        b'A'..=b'F' => 0x181,
        b'G'..=b'Z' => 0x101,
        b'a'..=b'f' => 0x182,
        b'g'..=b'z' => 0x102,
        0x21..=0x7E => 0x10,
        _ => 0,
    }
}

/// `_ftol` 0x5A0F98: `fistp` of a 64-bit integer under a truncating control word, returning the
/// low 32 bits — so a NaN (the "integer indefinite") gives 0.
fn ftol(x: f64) -> i32 {
    if x.is_nan() || x.abs() >= 9.223_372_036_854_776e18 { 0 } else { x.trunc() as i64 as i32 }
}

#[derive(Debug, Clone, Copy)]
struct Value {
    v: i32,
    /// false only for the port's unanswerable names (the module note)
    known: bool,
}

impl Value {
    fn num(v: i32) -> Value {
        Value { v, known: true }
    }
}

struct Evaluator {
    /// the operator stack `[ctx+0x21C]`: (operator, priority), bottomed by `' '` at priority −1
    /// (0x52F8E1…0x52F8EA)
    ops: Vec<(u8, i32)>,
    /// the value stack `[ctx+0x218]`
    vals: Vec<Value>,
    /// `[ctx+0x200]`, the default radix — 10 (0x52F74D with `ebx = 10`)
    radix: i32,
}

/// A syntax error in `0x52F830`: the whole expression is 0 (0x52FE6C…0x52FE95).
const SYNTAX_ERROR: Value = Value { v: 0, known: true };

impl Evaluator {
    fn new() -> Evaluator {
        Evaluator { ops: Vec::new(), vals: Vec::new(), radix: 10 }
    }

    /// `0x52F830`. `s` ends in at least four NULs.
    fn run(&mut self, s: &[u8]) -> Value {
        let at = |i: usize| s.get(i).copied().unwrap_or(0);
        // 0x52F84A…0x52F86A: when the first byte is not an operator, look for the first byte from
        // the second on that has a binary priority; if that is the terminator, the whole string
        // is one operand (spaces and all — a space has priority 0)
        let direct = if unary(at(0)) == 0 {
            let mut j = 1;
            while prec(at(j)) == 0 {
                j += 1;
            }
            at(j) == 0
        } else {
            at(0) == 0
        };
        if direct {
            return self.operand(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]);
        }
        self.ops.clear();
        self.vals.clear();
        self.ops.push((b' ', -1));
        let mut i = 0usize;
        let mut depth = 0i32;
        'operand: loop {
            // 0x52F905
            while at(i) == b' ' {
                i += 1;
            }
            let c = at(i);
            if c == b'(' || c == b'[' {
                // 0x52FD8B: pushed at its own priority, 1
                self.ops.push((c, prec(c)));
                depth += 1;
                i += 1;
                continue;
            }
            if matches!(c, b'+' | b'-' | b'~' | b'!' | b'<' | b'>') {
                // 0x52FD04 and the table at 0x52FF24: `+` is dropped; `<`/`>` look **two** bytes
                // on for a doubled form (0x52FD34 reads `[edi+1]` after `edi` has already moved)
                i += 1;
                let op = match c {
                    b'+' => continue 'operand,
                    b'-' => 0x8C,
                    b'~' => 0x8D,
                    b'!' => 0x8E,
                    b'<' if at(i + 1) == b'<' => {
                        i += 1;
                        0x91
                    }
                    b'<' => 0x8F,
                    b'>' if at(i + 1) == b'>' => {
                        i += 1;
                        0x92
                    }
                    _ => 0x90,
                };
                self.ops.push((op, unary(op)));
                continue;
            }
            // the operand token, 0x52F954…0x52F9EA
            let mut tok: Vec<u8> = Vec::new();
            if c == b'*' {
                tok.push(b'*');
                i += 1;
            } else if c != 0 {
                let mut quote = c == b'\'';
                if quote || unary(c) == 0 {
                    tok.push(c);
                    i += 1;
                    loop {
                        let d = at(i);
                        if d == 0 {
                            break;
                        }
                        if d == b'\'' {
                            quote = !quote;
                        }
                        if (!quote && prec(d) != 0) || d == b' ' {
                            break;
                        }
                        tok.push(d);
                        i += 1;
                    }
                }
            }
            // a name followed by `(` is a function (0x52F9EF…0x52FA24); any other operand is
            // followed by an **implied `*`**, and one followed by `[` by the index operator 0x8B
            let mut implied = 0u8;
            if at(i) == b'(' {
                if let Some(k) = FUNCTIONS.iter().position(|f| f.as_bytes() == tok.as_slice()) {
                    let op = 0x93 + k as u8;
                    self.ops.push((op, unary(op)));
                    continue;
                }
                implied = b'*';
            }
            if at(i) == b'[' {
                implied = 0x8B;
            }
            let v = self.operand(&tok);
            self.vals.push(v);
            while at(i) == b' ' {
                i += 1;
            }
            // closing brackets, 0x52FABF…0x52FB91
            loop {
                let c = at(i);
                if c != b')' && c != b']' {
                    break;
                }
                if depth == 0 {
                    return SYNTAX_ERROR;
                }
                self.reduce(prec(c));
                match self.ops.last() {
                    Some(&(b'(' | b'[', _)) => {
                        self.ops.pop();
                    }
                    _ => self.ops.push((c, prec(c))),
                }
                depth -= 1;
                i += 1;
            }
            if at(i) == 0 {
                break;
            }
            let op = if implied != 0 {
                implied
            } else {
                // 0x52FBA8: the operator byte, skipping anything at or below a space (signed, so a
                // byte above 0x7F is skipped too)
                let mut e = at(i) as i8;
                i += 1;
                while e <= 0x20 && e != 0 {
                    e = at(i) as i8;
                    i += 1;
                }
                let e = e as u8;
                // the two-byte forms, the table at 0x52FE98
                let second = at(i);
                let two = match (e, second) {
                    (b'/', b'/') | (b'%', b'%') => Some(b'%'),
                    (b'<', b'<') => Some(0x80),
                    (b'<', b'=') => Some(0x82),
                    (b'<', b'>') => Some(0x85),
                    (b'>', b'>') => Some(0x81),
                    (b'>', b'=') => Some(0x83),
                    (b'=', b'=') => Some(0x84),
                    (b'!', b'=') => Some(0x85),
                    (b'!', b':') => Some(0x89),
                    (b'!', b'?') => Some(0x8A),
                    (b'&', b'&') | (b'*', b'*') => Some(0x86),
                    (b'|', b'|') | (b'+', b'+') => Some(0x87),
                    (b'-', b'-') => Some(0x88),
                    _ => None,
                };
                match two {
                    Some(t) => {
                        i += 1;
                        t
                    }
                    // UNREACHABLE (2026-09-30): a byte above 0x7F indexes the tables with
                    // a negative offset (`movsx edx, byte [edi]` 0x52F84A, `[edx+esi+0x100]`
                    // 0x52F850, `[edx+esi]` 0x52F85F) — the heap before the context object.
                    // Taken here as no operator. No shipped expression holds such a byte: all 98
                    // `.lay` and 75 `.ctr` files are pure ASCII (checked byte by byte), and none
                    // of the 59 461 texts of the layout sweep has one; the only non-ASCII files
                    // of the front end are the `Locale` strings, which are spliced (`%name%`,
                    // 0x526B60) and never evaluated.
                    None if e > 0x7F => 0,
                    None => e,
                }
            };
            // 0x52FCAC: priority 1 or less (a separator, a bracket, a name) is a syntax error
            let p = prec(op);
            if p <= 1 {
                return SYNTAX_ERROR;
            }
            self.reduce(p);
            self.ops.push((op, p));
        }
        // 0x52FDDC
        if depth != 0 {
            return SYNTAX_ERROR;
        }
        self.reduce(0);
        // 0x52FDEB…0x52FE62: the top value is the result; a stack that holds more than one is
        // reported (error 7) but not refused
        self.vals.pop().unwrap_or(Value::num(0))
    }

    /// `0x52FFA0` — apply every stacked operator of higher priority, or of equal priority unless
    /// that priority is 14 (the unary ones group to the right, the rest to the left).
    fn reduce(&mut self, p: i32) {
        loop {
            let Some(&(op, tp)) = self.ops.last() else { return };
            if !(tp > p || (tp == p && p != 14)) {
                return;
            }
            if op >= 0x8C {
                let a = self.vals.pop().unwrap_or(Value::num(0));
                self.vals.push(Value { v: Self::unary_op(op, a.v), known: a.known });
            } else {
                let b = self.vals.pop().unwrap_or(Value::num(0));
                let a = self.vals.pop().unwrap_or(Value::num(0));
                self.vals.push(Self::binary_op(op, a, b));
            }
            self.ops.pop();
        }
    }

    /// The unary arms of the table at 0x530890.
    fn unary_op(op: u8, v: i32) -> i32 {
        let byte = |f: &dyn Fn(i32) -> i32| if v as u32 & 0xFFFF_FF00 != 0 { 0 } else { f(v) };
        match op {
            0x8C => v.wrapping_neg(),                 // 0x5305BC
            0x8D => !v,                               // 0x5305CD
            0x8E => (v == 0) as i32,                  // 0x5305DE
            0x8F => (v >> 16) & 0xFFFF,               // 0x53060D `<x`: the high word
            0x90 => v & 0xFFFF,                       // 0x530625 `>x`: the low word
            0x91 => v.wrapping_shl(1),                // 0x5305F3
            0x92 => v >> 1,                           // 0x530600
            0x93 => v.wrapping_neg().max(v) & 0x7FFF_FFFF, // abs, 0x5300D4: `neg` then `and 0x7FFFFFFF`
            0x94 => byte(&|c| ctype(c) & 0x103),      // isalpha
            0x95 => byte(&|c| ctype(c) & 0x001),      // isupper
            0x96 => byte(&|c| ctype(c) & 0x002),      // islower
            0x97 => byte(&|c| ctype(c) & 0x004),      // isdigit
            0x98 => byte(&|c| ctype(c) & 0x080),      // isxdigit
            0x99 => byte(&|c| ctype(c) & 0x107),      // isalnum
            0x9A => byte(&|c| ctype(c) & 0x008),      // isspace
            0x9B => byte(&|c| ctype(c) & 0x010),      // ispunct
            0x9C => byte(&|c| ctype(c) & 0x020),      // iscntrl
            0x9D => byte(&|c| ctype(c) & 0x157),      // isprint
            0x9E => byte(&|c| ctype(c) & 0x117),      // isgraph
            0x9F => byte(&|c| (c < 0x80) as i32),     // isascii, 0x5304E2
            0xA0 => if v as u32 & 0xFFFF_FF00 != 0 { v } else { (v as u8).to_ascii_lowercase() as i32 }, // 0x53050A
            0xA1 => if v as u32 & 0xFFFF_FF00 != 0 { v } else { (v as u8).to_ascii_uppercase() as i32 }, // 0x53052F
            0xA2 => v & 0x7F,                         // toascii, 0x530554
            // intsin / intcos: `fild`, × 2π/1024 ([0x5BB2A8]), `fsin`/`fcos`, × 65536 ([0x5B4B38]),
            // `_ftol` (0x530562 / 0x530584)
            0xA3 => ftol((v as f64 * 0.006_135_922_572_265_625).sin() * 65536.0),
            0xA4 => ftol((v as f64 * 0.006_135_922_572_265_625).cos() * 65536.0),
            0xA5 => ftol((v as f64).sqrt()),          // 0x5305A6
            _ => v,
        }
    }

    /// The binary arms of the table at 0x530890. Every one writes the left slot; an operator with
    /// no arm (a bracket pushed by the odd path of 0x52FB00) is error 7 and leaves it alone.
    fn binary_op(op: u8, a: Value, b: Value) -> Value {
        let known = a.known && b.known;
        let (x, y) = (a.v, b.v);
        let v = match op {
            b'*' => x.wrapping_mul(y),
            // `idiv` — truncating — and a zero divisor leaves the left value as it is (0x530652)
            b'/' => if y == 0 { x } else { x.wrapping_div(y) },
            b'%' => if y == 0 { x } else { x.wrapping_rem(y) },
            b'+' | b'.' | 0x8B => x.wrapping_add(y),
            b'-' => x.wrapping_sub(y),
            0x80 => x.wrapping_shl(y as u32),  // `shl edx, cl` — the count is taken mod 32
            0x81 => x.wrapping_shr(y as u32),  // `sar`
            b'<' => (x < y) as i32,
            0x82 => (x <= y) as i32,
            b'>' => (x > y) as i32,
            0x83 => (x >= y) as i32,
            b'=' | 0x84 => (x == y) as i32,
            b'#' | 0x85 => (x != y) as i32,
            b'&' => x & y,
            b'^' | b'~' => x ^ y,
            b'|' | b'!' => x | y,
            0x86 => (x != 0 && y != 0) as i32,
            0x87 => (x != 0 || y != 0) as i32,
            0x88 => ((x != 0) != (y != 0)) as i32,
            // `c ? a` is `a` when `c` holds and the sentinel when not; `x : b` is `b` when `x` is
            // the sentinel. Both sides are always evaluated, and the two group to the left at the
            // same priority, so `c1?a:c2?b:d` is `((c1?a):c2)?b:d` (0x53081F / 0x53083A)
            b'?' | 0x89 => {
                if !a.known {
                    return Value { v: 0, known: false };
                }
                return if x != 0 { b } else { Value::num(NOT_TAKEN) };
            }
            b':' | 0x8A => {
                if !a.known {
                    return Value { v: 0, known: false };
                }
                return if x == NOT_TAKEN { b } else { a };
            }
            _ => x,
        };
        Value { v, known }
    }

    /// `0x5309E0` — one operand token.
    fn operand(&mut self, t: &[u8]) -> Value {
        let at = |i: usize| t.get(i).copied().unwrap_or(0);
        let c0 = at(0);
        // 0x5309FC: a byte above `@` (signed) is a name while the radix is at most 10
        if (c0 as i8) > 0x40 && self.radix <= 10 {
            return self.symbol(t);
        }
        if at(1) == 0 {
            return self.single(t);
        }
        if (c0 as i8) >= 0x30 {
            // 0x530AE7: `0x…` is hex while the radix is below 0x22, `@…` octal, else the radix
            if c0 == b'0' && at(1) == b'x' && self.radix < 0x22 {
                return self.digits(t, 2, 16);
            }
            if c0 == b'@' {
                return self.digits(t, 1, 8);
            }
            return self.digits(t, 0, self.radix);
        }
        match c0 {
            b'$' | b'&' => self.digits(t, 1, 16),
            b'%' => self.digits(t, 1, 2),
            b'"' | b'\'' => self.character(t),
            // the NUL arm of the table at 0x530F50 is error 7 and 0 (0x530ACC)
            0 => Value::num(0),
            _ => self.digits(t, 1, 0),
        }
    }

    /// The digit readers 0x530B11 (radix 10), 0x530BAD (16) and 0x530C42 (any other). A string
    /// that does not end where the digits do goes to `0x530CC3`.
    fn digits(&mut self, t: &[u8], mut p: usize, r: i32) -> Value {
        let at = |i: usize| t.get(i).copied().unwrap_or(0);
        let conv = |c: u8| {
            let d = c.wrapping_sub(0x30);
            if d < 10 { d } else { (c | 0x20).wrapping_sub(0x57) }
        };
        let mut v: i32 = 0;
        match r {
            10 => {
                let d = at(p).wrapping_sub(0x30);
                p += 1;
                if d >= 10 {
                    return self.not_a_number(t);
                }
                v = d as i32;
                loop {
                    let d = at(p).wrapping_sub(0x30);
                    p += 1;
                    if d >= 10 {
                        break;
                    }
                    v = v.wrapping_mul(10).wrapping_add(d as i32);
                }
                // more than ten digits is error 0xA, reported and ignored (0x530BA5)
                if at(p - 1) != 0 {
                    return self.not_a_number(t);
                }
            }
            16 => {
                let mut d = conv(at(p));
                p += 1;
                if d >= 16 {
                    return self.not_a_number(t);
                }
                loop {
                    v = v.wrapping_shl(4).wrapping_add(d as i32);
                    d = conv(at(p));
                    p += 1;
                    if d < 16 {
                        continue;
                    }
                    // one `_` between digits is skipped (0x530C03)
                    if at(p - 1) == b'_' {
                        d = conv(at(p));
                        p += 1;
                    }
                    if d >= 16 {
                        break;
                    }
                }
                if at(p - 1) != 0 {
                    return self.not_a_number(t);
                }
            }
            _ => {
                // 0x530C42…0x530C86. The loop leaves its pointer **on** the terminator and the
                // test after it reads the byte before — the last digit, never 0 — so this reader
                // always falls through to 0x530CC3: a `%1010`, `@17`, `1010b` or `17o` is never
                // a number in the original.
                while at(p) != 0 {
                    let d = conv(at(p));
                    p += 1;
                    if (d as i32) < r {
                        v = v.wrapping_mul(r).wrapping_add(d as i32);
                    } else if at(p - 1) != b'_' {
                        break;
                    }
                }
                let _ = v;
                return self.not_a_number(t);
            }
        }
        Value::num(v)
    }

    /// `0x530CC3` — a token that is not a number. One that starts with a digit may carry a radix
    /// suffix (`b` 2, `d` 10, `h` 16, `o`/`q` 8; the table at 0x530F8C), read again without it
    /// under that radix; anything else is a name.
    fn not_a_number(&mut self, t: &[u8]) -> Value {
        if ctype(t.first().copied().unwrap_or(0) as i32) & 4 != 0 {
            let n = t.iter().position(|&c| c == 0).unwrap_or(t.len());
            let last = t[n - 1];
            let r = match ((last as i8 as i32) | 0x20) - 0x62 {
                0 => Some(2),
                2 => Some(10),
                6 => Some(16),
                0xD | 0xF => Some(8),
                _ => None,
            };
            if let Some(r) = r {
                let saved = self.radix;
                self.radix = r;
                let v = self.operand(&t[..n - 1]);
                self.radix = saved;
                return v;
            }
        }
        self.symbol(t)
    }

    /// `0x530DF6` — a one-byte token: a digit below the radix, the location counter `*`, or a name.
    fn single(&mut self, t: &[u8]) -> Value {
        let c = t.first().copied().unwrap_or(0);
        let d = c.wrapping_sub(0x30);
        let d = if d < 10 { d } else { (c | 0x20).wrapping_sub(0x57) };
        if (d as i32) < self.radix {
            return Value::num(d as i32);
        }
        if c == b'*' {
            // `[ctx+0x23C]`, the location counter: zeroed by the constructor and never written
            return Value::num(0);
        }
        self.symbol(t)
    }

    /// `0x530DB8` — a name. `^A`…`^Z` are 1…26; the symbol table is always empty in the game, so
    /// every other name is 0 — `narg` included (0x530ECE: `_stricmp(name, "narg")`, then
    /// `eax = [[arg1+0x220]]`, −1 → 0, 0x530EE0…0x530EEF), which reads the dword at the end of the
    /// operator buffer, `[ctx+0x220]` being the buffer's end (0x52F899, the first operator cell sits
    /// at `end − 8`, 0x52F8CD): a dword past the buffer that the evaluator never writes (heap
    /// contents), taken as 0. UNREACHABLE (2026-09-30): the name `narg` (any case) occurs
    /// in none of the 98 `.lay` / 75 `.ctr` files (48 428 values) and in none of the 59 461
    /// evaluated texts of the layout sweep.
    fn symbol(&mut self, t: &[u8]) -> Value {
        let n = t.iter().position(|&c| c == 0).unwrap_or(t.len());
        let t = &t[..n];
        if t.is_empty() {
            return Value::num(0);
        }
        if t[0] == b'^' && t.len() == 2 {
            let k = (t[1] | 0x20).wrapping_sub(0x60);
            if k < 0x1B {
                return Value::num(k as i32);
            }
        }
        if t.windows(UNKNOWN.len()).any(|w| w == UNKNOWN.as_bytes()) {
            return Value { v: 0, known: false };
        }
        Value::num(0)
    }

    /// `0x530A4D` with `0x530FB0` — a character constant, up to four bytes packed big-endian.
    fn character(&mut self, t: &[u8]) -> Value {
        let (bytes, closed) = parse_quoted(t);
        // 0x530A6A: not closed by a quote is error 0x1B; more than four bytes is error 0xA — 0
        if !closed || bytes.len() > 4 {
            return Value::num(0);
        }
        Value::num(bytes.iter().fold(0i32, |v, &b| v.wrapping_shl(8) | b as i32))
    }
}

/// `0x530FB0` — the body of a quoted token and whether it was closed by its quote. `'…'` takes
/// bytes as they are with `''` for a quote; `"…"` knows `\n \t \b \r \f \\ \< \' \"` and three
/// octal digits, `""` for a quote, and `<expr>` for an expression's value in decimal (`%ld`,
/// 0x5311E6). The original evaluates `<expr>` on the same context (0x5311B9 `call 0x52F830` with
/// the parse's own `esi`) and so resets the stacks of any parse it sits inside (0x52F893…0x52F8D3
/// rewrite `[ctx+0x208…0x21C]`); this evaluates it on its own. UNREACHABLE (2026-09-30): a
/// quoted token needs a `'` or `"`, and none of the 48 428 values of the 98 `.lay` / 75 `.ctr`
/// files holds either (0 hits), nor does any of the 59 461 evaluated texts of the layout sweep.
fn parse_quoted(t: &[u8]) -> (Vec<u8>, bool) {
    let at = |i: usize| t.get(i).copied().unwrap_or(0);
    let mut out = Vec::new();
    let mut p = 1usize;
    if at(0) == b'\'' {
        loop {
            let c = at(p);
            p += 1;
            if c == 0 {
                return (out, false);
            }
            out.push(c);
            if c == b'\'' {
                if at(p) == b'\'' {
                    p += 1;
                    continue;
                }
                out.pop();
                return (out, true);
            }
        }
    }
    loop {
        let c = at(p);
        p += 1;
        match c {
            0 => return (out, false),
            b'\\' => {
                let e = at(p);
                p += 1;
                let simple = match e {
                    b'n' => Some(0x0A),
                    b't' => Some(0x09),
                    b'b' => Some(0x08),
                    b'r' => Some(0x0D),
                    b'f' => Some(0x0C),
                    b'\\' | b'<' | b'\'' | b'"' => Some(e),
                    _ => None,
                };
                if let Some(s) = simple {
                    out.push(s);
                    // an escaped `"` is taken for the closing quote all the same (0x53125D)
                    if e == b'"' && at(p) != b'"' {
                        out.pop();
                        return (out, true);
                    }
                    continue;
                }
                let d = e.wrapping_sub(b'0');
                if d < 8 {
                    let mut v = d;
                    let d2 = at(p).wrapping_sub(b'0');
                    p += 1;
                    if d2 < 8 {
                        v = v.wrapping_shl(3).wrapping_add(d2);
                        let d3 = at(p).wrapping_sub(b'0');
                        p += 1;
                        if d3 < 8 {
                            v = v.wrapping_shl(3).wrapping_add(d3);
                            p += 1;
                        }
                    }
                    out.push(v);
                }
                // the byte that ended the escape is read again (`dec ebp`, 0x5310DC)
                p -= 1;
            }
            b'<' => {
                let s = p;
                while at(p) != 0 && at(p) != b'>' {
                    p += 1;
                }
                let inner = String::from_utf8_lossy(&t[s.min(t.len())..p.min(t.len())]).into_owned();
                if at(p) == b'>' {
                    p += 1;
                }
                let v = evaluate(&inner).unwrap_or(0);
                out.extend_from_slice(v.to_string().as_bytes());
            }
            b'"' => {
                if at(p) == b'"' {
                    p += 1;
                    out.push(b'"');
                    continue;
                }
                return (out, true);
            }
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Macro {
        Macro::Undefined
    }

    fn e(s: &str) -> i32 {
        eval(s, &none).unwrap()
    }

    #[test]
    fn arithmetic_is_32_bit_integer() {
        // straight out of `MovieCircle.ctr`
        assert_eq!(e("640-(113+112)-79"), 336);
        assert_eq!(e("548-(158+100)-116"), 174);
        assert_eq!(e("192-112"), 80);
        assert_eq!(e("146-100+12"), 58);
        // `idiv` truncates toward zero, and `(0-w)/2*align` is `((0-w)/2)*align`
        assert_eq!(e("(0-57)/2*1"), -28);
        assert_eq!(e("(0-57)/2*2"), -56);
        // a hex literal is a 32-bit pattern
        assert_eq!(e("0x80000000"), i32::MIN);
        assert_eq!(e("0xffffffff"), -1);
        // a zero divisor leaves the left value alone
        assert_eq!(e("7/0"), 7);
        assert_eq!(e("7%0"), 7);
        // the `.lay` sums
        assert_eq!(e("440+8"), 448);
        assert_eq!(e("260+95"), 355);
    }

    #[test]
    fn precedence_is_the_tables() {
        assert_eq!(e("2+3*4"), 14);
        assert_eq!(e("1+6&3"), 3); // `&` below `+`
        assert_eq!(e("1<<4+1"), 32); // `<<` below `+`
        assert_eq!(e("5-3-1"), 1); // left to right
        assert_eq!(e("-2*3"), -6);
        assert_eq!(e("--4"), 4); // two unary minuses at the start
        assert_eq!(e("1--1"), 0); // …but between operands `--` is the logical exclusive or
        assert_eq!(e("3**0"), 0); // `**` is `&&`
        assert_eq!(e("0++2"), 1); // `++` is `||`
        assert_eq!(e("2=2"), 1);
        assert_eq!(e("2#3"), 1);
        assert_eq!(e("2<>2"), 0);
        assert_eq!(e("5//3"), 2); // `//` is the remainder
        assert_eq!(e("6!1"), 7); // a binary `!` is `|`
        assert_eq!(e("6~3"), 5); // a binary `~` is `^`
    }

    #[test]
    fn unary_forms() {
        assert_eq!(e("<0x12345678"), 0x1234);
        assert_eq!(e(">0x12345678"), 0x5678);
        assert_eq!(e("!0"), 1);
        assert_eq!(e("~0"), -1);
        assert_eq!(e("+5"), 5);
        assert_eq!(e("abs(0-5)"), 5);
        assert_eq!(e("sqrt(17)"), 4);
        // 256 × the constant at [0x5BB2A8] falls just short of π/2, and `_ftol` truncates
        assert_eq!(e("intsin(256)"), 65535);
        assert_eq!(e("intsin(0)"), 0);
        assert_eq!(e("isdigit(48)"), 4);
        assert_eq!(e("toupper(97)"), 65);
    }

    #[test]
    fn the_ternary_is_two_operators() {
        // `GoToButton.ctr`'s caption colours, `data.medium` already expanded to its text
        assert_eq!(e("0?0x00000000:0xffffffff"), -1);
        assert_eq!(e("1?0x00000000:0xffffffff"), 0);
        assert_eq!(e("(4&4)?0xffffffff:(4&2)?0xffffffff:0x00000000"), -1);
        assert_eq!(e("(2&4)?0xffffffff:(2&2)?0xffffffff:0x00000000"), -1);
        assert_eq!(e("(1&4)?0xffffffff:(1&2)?0xffffffff:0x00000000"), 0);
        // left-grouped: `1?0:0?5:9` is `((1?0):0)?5:9`, so 9, where C would say 0
        assert_eq!(e("1?0:0?5:9"), 9);
        // `&` binds tighter than `?`: `ColorBox.ctr`'s `data.nolabel&1?5:…`
        assert_eq!(e("3&1?5:7"), 5);
    }

    #[test]
    fn names_and_numbers_the_way_the_operand_reader_takes_them() {
        // an undefined name is 0 and the expression goes on
        assert_eq!(e("data.area"), 0);
        assert_eq!(e("data.area+5"), 5);
        // a decimal point becomes `X` before evaluation, so `1.5` is the name `1X5`: 0
        assert_eq!(e("1.5"), 0);
        // radix prefixes and suffixes: hex works, binary and octal never do (0x530C81)
        assert_eq!(e("$1f"), 31);
        assert_eq!(e("&10"), 16);
        assert_eq!(e("1fh"), 31);
        assert_eq!(e("12d"), 12);
        assert_eq!(e("%101"), 0);
        assert_eq!(e("101b"), 0);
        assert_eq!(e("0x1_f"), 31);
        // character constants, big-endian
        assert_eq!(e("'AB'"), 0x4142);
        assert_eq!(e("^C"), 3);
        // a trailing space on a lone operand makes it a name (0x52F84A takes the whole string)
        assert_eq!(e("16 "), 0);
        assert_eq!(e("16 +1"), 17);
        // syntax errors are 0 for the whole expression
        assert_eq!(e("(5"), 0);
        assert_eq!(e("5)"), 0);
        assert_eq!(e("5,6"), 0);
        // an empty operand is 0
        assert_eq!(e(""), 0);
        assert_eq!(e("(0-)/2"), 0);
    }

    #[test]
    fn expansion_is_textual() {
        let r = |n: &str| match n {
            "data.width" => Macro::Text("440+8".into()),
            "label.size.width" => Macro::Text("40".into()),
            "data.align" => Macro::Text("1".into()),
            "a" => Macro::Text("b".into()),
            "b" => Macro::Text("a".into()),
            "w" => Macro::Unanswerable,
            _ => Macro::Undefined,
        };
        // the macro's text is spliced in, not its value: `440+8/2` is 444
        assert_eq!(eval("data.width/2", &r), Some(444));
        assert_eq!(eval("(data.width-label.size.width)/2", &r), Some(204));
        assert_eq!(eval("(0-label.size.width)/2*data.align", &r), Some(-20));
        // a macro met again inside itself empties the whole expansion: 0
        assert_eq!(expand("a+1", &r), "");
        assert_eq!(eval("a+1", &r), Some(0));
        // a run that is exactly `##` is dropped; one with anything else in it is not
        assert_eq!(expand("x##y", &r), "xy");
        assert_eq!(expand("1##2", &r), "1##2");
        // an unanswerable name has no value — unless it sits in the branch not taken
        assert_eq!(eval("w+1", &r), None);
        assert_eq!(eval("0?w:5", &r), Some(5));
        assert_eq!(eval("1?5:w", &r), Some(5));
        assert_eq!(eval("w?5:6", &r), None);
    }

    #[test]
    fn a_pair_is_cut_at_its_first_comma() {
        assert_eq!(eval_pair("data.width-1,0", &|n| if n == "data.width" { Macro::Text("100".into()) } else { Macro::Undefined }), Some((99, 0)));
        // `size=150,`: the second half is empty, which is 0
        assert_eq!(eval_pair("150,", &none), Some((150, 0)));
        // no comma: not evaluated at all (0x527BB7)
        assert_eq!(eval_pair("454", &none), None);
        // everything after the first comma is the second half — here a syntax error, so 0
        assert_eq!(eval_pair("1,2,3", &none), Some((1, 0)));
    }
}
