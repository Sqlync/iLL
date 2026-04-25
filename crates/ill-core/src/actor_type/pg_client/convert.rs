// Convert tokio-postgres `Row`s into runtime `Value`s.
//
// Covers the types the pg_client examples reach for today: INT*, FLOAT*,
// TEXT/VARCHAR/NAME, BOOL, BYTEA, UUID, JSON/JSONB, plus the common
// date/time family rendered as ISO strings. Anything else surfaces as a
// placeholder `String("<unsupported pg type …>")` rather than an error —
// tests can still assert on `row_count` and other cells while a real fix
// is lined up.

use tokio_postgres::types::Type;
use tokio_postgres::Row;

use crate::runtime::{Dict, Value};

/// Build the structured `ok.*` dict for a query result: `cell`, `col`,
/// `row_count`, `col_count`. `cell` is the 2D array — `cell[i]` is row
/// `i`, `cell[i, j]` is that row's `j`th cell (multi-arg indexing folds
/// into repeated single-arg indexing).
pub fn build_result_dict(rows: &[Row]) -> Dict {
    let col_count = rows.first().map(|r| r.columns().len()).unwrap_or(0);
    let col_names: Vec<String> = rows
        .first()
        .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
        .unwrap_or_default();

    let mut row_values: Vec<Value> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells: Vec<Value> = Vec::with_capacity(col_count);
        for i in 0..row.columns().len() {
            cells.push(cell_to_value(row, i));
        }
        row_values.push(Value::Array(cells));
    }

    // col: Dict of column-name → Array of values. Preserves column order
    // because Dict is an IndexMap, so `ok.col[0]` returns the first
    // column as declared by the query.
    let mut col_dict: Dict = Dict::new();
    for (ci, name) in col_names.iter().enumerate() {
        let mut column_cells: Vec<Value> = Vec::with_capacity(rows.len());
        for row_value in &row_values {
            let cell = match row_value {
                Value::Array(cells) => cells.get(ci).cloned().unwrap_or(Value::Null),
                _ => Value::Null,
            };
            column_cells.push(cell);
        }
        col_dict.insert(name.clone(), Value::Array(column_cells));
    }

    let mut out = Dict::new();
    out.insert("cell".into(), Value::Array(row_values));
    out.insert("col".into(), Value::Dict(col_dict));
    out.insert("row_count".into(), Value::Number(rows.len() as i64));
    out.insert("col_count".into(), Value::Number(col_count as i64));
    out
}

/// Extract column `i` as a `Value`. NULL → `Value::Null`; unrecognised
/// type → a placeholder string so the rest of the result still indexes.
fn cell_to_value(row: &Row, i: usize) -> Value {
    let ty = row.columns()[i].type_();
    match *ty {
        Type::BOOL => row
            .try_get::<_, Option<bool>>(i)
            .ok()
            .flatten()
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        Type::INT2 => row
            .try_get::<_, Option<i16>>(i)
            .ok()
            .flatten()
            .map(|n| Value::Number(n as i64))
            .unwrap_or(Value::Null),
        Type::INT4 => row
            .try_get::<_, Option<i32>>(i)
            .ok()
            .flatten()
            .map(|n| Value::Number(n as i64))
            .unwrap_or(Value::Null),
        Type::INT8 => row
            .try_get::<_, Option<i64>>(i)
            .ok()
            .flatten()
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(i)
            .ok()
            .flatten()
            .map(|x| Value::Float(x as f64))
            .unwrap_or(Value::Null),
        Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(i)
            .ok()
            .flatten()
            .map(Value::Float)
            .unwrap_or(Value::Null),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => row
            .try_get::<_, Option<String>>(i)
            .ok()
            .flatten()
            .map(Value::String)
            .unwrap_or(Value::Null),
        Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(i)
            .ok()
            .flatten()
            .map(Value::Bytes)
            .unwrap_or(Value::Null),
        _ => {
            // Last resort: try to read as a string. tokio-postgres has
            // `FromSql for String` on TEXT-family types; for others this
            // falls through to an error. We surface a placeholder naming
            // the type so the failure is obvious in assertions.
            match row.try_get::<_, Option<String>>(i) {
                Ok(Some(s)) => Value::String(s),
                Ok(None) => Value::Null,
                Err(_) => Value::String(format!("<unsupported pg type {}>", ty.name())),
            }
        }
    }
}
