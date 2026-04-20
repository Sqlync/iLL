// Assert evaluator. Evaluates both sides, applies the op, returns a result
// that reports the actual values back for failure messages.
//
// Phase 5 supports the comparisons that exec examples need:
//   Eq, NotEq, Gt, Gte, Lt, Lte
// Contains, NotContains, Matches, NotMatches return an Eval error for now —
// they land when the first exec example needs them.

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
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Record(r) => !r.is_empty(),
        Value::Bytes(b) => !b.is_empty(),
        Value::Atom(_) => true,
        Value::Unit => false,
    }
}

fn compare(left: &Value, right: &Value, op: ComparisonOp) -> Result<bool, RuntimeError> {
    use ComparisonOp::*;
    match op {
        Eq => Ok(values_equal(left, right)),
        NotEq => Ok(!values_equal(left, right)),
        Gt | Gte | Lt | Lte => {
            let ord = compare_ord(left, right)?;
            Ok(match op {
                Gt => ord == std::cmp::Ordering::Greater,
                Gte => ord != std::cmp::Ordering::Less,
                Lt => ord == std::cmp::Ordering::Less,
                Lte => ord != std::cmp::Ordering::Greater,
                _ => unreachable!(),
            })
        }
        Contains | NotContains | Matches | NotMatches => Err(RuntimeError::Eval(format!(
            "comparison operator {op:?} not yet supported in runtime"
        ))),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    a == b
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
}
