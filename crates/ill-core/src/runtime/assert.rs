// Assert evaluator. Evaluates both sides, applies the op, returns a result
// that reports the actual values back for failure messages.

use crate::ast::{Assert, ComparisonOp};

use super::eval::{eval, Scope};
use super::{RuntimeError, Value};

pub struct AssertResult {
    pub passed: bool,
    pub left: Value,
    pub right: Option<Value>,
    pub op: Option<ComparisonOp>,
}

pub fn eval_assert(a: &Assert, scope: &Scope) -> Result<AssertResult, RuntimeError> {
    let left = eval(&a.left, scope)?;

    let Some(op) = a.op else {
        // Bare `assert expr` — truthiness.
        let passed = is_truthy(&left);
        return Ok(AssertResult {
            passed,
            left,
            right: None,
            op: None,
        });
    };

    let right = match &a.right {
        Some(r) => eval(r, scope)?,
        None => {
            return Err(RuntimeError::Eval(
                "assert has comparison operator but no right-hand side".into(),
            ))
        }
    };

    let passed = compare(&left, &right, op)?;
    Ok(AssertResult {
        passed,
        left,
        right: Some(right),
        op: Some(op),
    })
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => *n != 0,
        Value::Float(x) => *x != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Dict(fields) => !fields.is_empty(),
        Value::Bytes(b) => !b.is_empty(),
        Value::Atom(_) => true,
        Value::Null => false,
    }
}

fn compare(left: &Value, right: &Value, op: ComparisonOp) -> Result<bool, RuntimeError> {
    use ComparisonOp::*;
    match op {
        Eq => Ok(values_eq(left, right)),
        NotEq => Ok(!values_eq(left, right)),
        Gt => Ok(compare_ord(left, right)? == std::cmp::Ordering::Greater),
        Gte => Ok(compare_ord(left, right)? != std::cmp::Ordering::Less),
        Lt => Ok(compare_ord(left, right)? == std::cmp::Ordering::Less),
        Lte => Ok(compare_ord(left, right)? != std::cmp::Ordering::Greater),
        Contains => contains(left, right),
        NotContains => contains(left, right).map(|b| !b),
        Matches => matches_regex(left, right),
        NotMatches => matches_regex(left, right).map(|b| !b),
    }
}

/// Equality with cross-type coercion for `String` vs `Bytes`. MQTT publish
/// payloads come back as `Value::Bytes`; the natural way for users to write
/// assertions on text payloads is `assert ok.payload == "hello"`. Treating
/// the two as equal when the bytes are valid UTF-8 matching the string lets
/// that pattern work without a separate `~b` literal. Non-UTF-8 bytes can
/// only equal another `Bytes` value.
fn values_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(s), Value::Bytes(b)) | (Value::Bytes(b), Value::String(s)) => {
            s.as_bytes() == b.as_slice()
        }
        _ => left == right,
    }
}

/// `matches` compiles the right-hand side as a regex and tests whether the
/// left-hand string matches it. Regex is compiled per assertion.
fn matches_regex(subject: &Value, pattern: &Value) -> Result<bool, RuntimeError> {
    let (Value::String(s), Value::String(p)) = (subject, pattern) else {
        return Err(RuntimeError::Eval(format!(
            "`matches` requires string left and string regex right, got {} and {}",
            subject.type_name(),
            pattern.type_name()
        )));
    };
    let re = regex::Regex::new(p)
        .map_err(|e| RuntimeError::Eval(format!("invalid regex `{p}`: {e}")))?;
    Ok(re.is_match(s))
}

/// `contains` works on strings (substring) and arrays (membership, by
/// `Value`-equality). Haystack types outside those two are an error; needle
/// type mismatches inside an array fall out as `false` via `PartialEq`.
fn contains(haystack: &Value, needle: &Value) -> Result<bool, RuntimeError> {
    match (haystack, needle) {
        (Value::String(h), Value::String(n)) => Ok(h.contains(n.as_str())),
        (Value::Array(items), needle) => Ok(items.iter().any(|item| item == needle)),
        (haystack, needle) => Err(RuntimeError::Eval(format!(
            "cannot test whether {} contains {}",
            haystack.type_name(),
            needle.type_name()
        ))),
    }
}

fn compare_ord(a: &Value, b: &Value) -> Result<std::cmp::Ordering, RuntimeError> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Ok(x.cmp(y)),
        (Value::String(x), Value::String(y)) => Ok(x.cmp(y)),
        _ => Err(RuntimeError::Eval(format!(
            "cannot order-compare {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Ident, Span};

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.into(),
            span: span(),
        }
    }

    fn assert_stmt(left: Expr, op: Option<ComparisonOp>, right: Option<Expr>) -> Assert {
        Assert {
            annotation: None,
            left,
            op,
            right,
            span: span(),
        }
    }

    #[test]
    fn eq_passes() {
        let s = Scope::new();
        let a = assert_stmt(
            Expr::Number(1),
            Some(ComparisonOp::Eq),
            Some(Expr::Number(1)),
        );
        let r = eval_assert(&a, &s).unwrap();
        assert!(r.passed);
    }

    #[test]
    fn neq_fails_on_equal() {
        let s = Scope::new();
        let a = assert_stmt(
            Expr::Number(1),
            Some(ComparisonOp::NotEq),
            Some(Expr::Number(1)),
        );
        let r = eval_assert(&a, &s).unwrap();
        assert!(!r.passed);
    }

    #[test]
    fn gt_numbers() {
        let s = Scope::new();
        let a = assert_stmt(
            Expr::Number(5),
            Some(ComparisonOp::Gt),
            Some(Expr::Number(3)),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);

        let a = assert_stmt(
            Expr::Number(1),
            Some(ComparisonOp::Gt),
            Some(Expr::Number(3)),
        );
        assert!(!eval_assert(&a, &s).unwrap().passed);
    }

    #[test]
    fn bare_assert_truthiness() {
        let s = Scope::new();
        let a = assert_stmt(Expr::Bool(true), None, None);
        assert!(eval_assert(&a, &s).unwrap().passed);

        let a = assert_stmt(Expr::Bool(false), None, None);
        assert!(!eval_assert(&a, &s).unwrap().passed);
    }

    #[test]
    fn ident_compared_against_literal() {
        let mut s = Scope::new();
        s.bind("x", Value::Number(42));
        let a = assert_stmt(
            Expr::Ident(ident("x")),
            Some(ComparisonOp::Eq),
            Some(Expr::Number(42)),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);
    }

    #[test]
    fn mismatched_types_order_errors() {
        let s = Scope::new();
        let a = assert_stmt(
            Expr::Number(1),
            Some(ComparisonOp::Gt),
            Some(Expr::Bool(true)),
        );
        assert!(eval_assert(&a, &s).is_err());
    }

    fn str_lit(s: &str) -> Expr {
        Expr::StringLit(crate::ast::StringLit {
            fragments: vec![crate::ast::StringFragment::Text(s.into())],
            span: span(),
        })
    }

    #[test]
    fn contains_substring() {
        let s = Scope::new();
        let a = assert_stmt(
            str_lit("alice@example.com"),
            Some(ComparisonOp::Contains),
            Some(str_lit("example.com")),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);

        let a = assert_stmt(
            str_lit("alice@example.com"),
            Some(ComparisonOp::NotContains),
            Some(str_lit("nope")),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);
    }

    #[test]
    fn contains_array_member() {
        let mut s = Scope::new();
        s.bind(
            "names",
            Value::Array(vec![
                Value::String("alice".into()),
                Value::String("bob".into()),
            ]),
        );
        let a = assert_stmt(
            Expr::Ident(ident("names")),
            Some(ComparisonOp::Contains),
            Some(str_lit("alice")),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);

        let a = assert_stmt(
            Expr::Ident(ident("names")),
            Some(ComparisonOp::Contains),
            Some(str_lit("dave")),
        );
        assert!(!eval_assert(&a, &s).unwrap().passed);
    }

    #[test]
    fn contains_rejects_unsupported_types() {
        let s = Scope::new();
        // number contains number — meaningless
        let a = assert_stmt(
            Expr::Number(42),
            Some(ComparisonOp::Contains),
            Some(Expr::Number(4)),
        );
        assert!(eval_assert(&a, &s).is_err());
    }

    #[test]
    fn matches_regex() {
        let s = Scope::new();
        let a = assert_stmt(
            str_lit("charlie@example.org"),
            Some(ComparisonOp::Matches),
            Some(str_lit(r"^charlie@.+\.org$")),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);

        let a = assert_stmt(
            str_lit("charlie@example.org"),
            Some(ComparisonOp::NotMatches),
            Some(str_lit(r"^alice")),
        );
        assert!(eval_assert(&a, &s).unwrap().passed);
    }

    #[test]
    fn invalid_regex_errors() {
        let s = Scope::new();
        let a = assert_stmt(
            str_lit("hello"),
            Some(ComparisonOp::Matches),
            Some(str_lit("(")), // unterminated group
        );
        assert!(eval_assert(&a, &s).is_err());
    }
}
