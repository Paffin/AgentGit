//! A small backend-neutral SQL layer.
//!
//! Every query in [`super`] is written once, in the subset of SQL that SQLite
//! and PostgreSQL share:
//!
//! * `?1`-style numbered placeholders (rewritten to `$1` for PostgreSQL);
//! * `INSERT … ON CONFLICT … DO NOTHING / DO UPDATE`;
//! * `INSERT … RETURNING id` instead of `last_insert_rowid()`;
//! * 64-bit integers only, text, and blobs.
//!
//! A connection is anything implementing `Conn`: it executes a statement
//! with typed [`Param`]s and returns [`Row`]s of typed [`Value`]s. The layer is
//! deliberately thin, there is no query builder.

use std::borrow::Cow;

use super::MetaError;

/// The SQL dialect behind a `Conn`; only needed for the rare statement that
/// cannot be written in the common subset (for example `FOR UPDATE SKIP LOCKED`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// SQLite (embedded, one writer at a time).
    Sqlite,
    /// PostgreSQL.
    Postgres,
}

/// A bound statement parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum Param<'a> {
    /// SQL `NULL`.
    Null,
    /// A 64-bit integer.
    Int(i64),
    /// Text (never contains a NUL byte on PostgreSQL).
    Text(Cow<'a, str>),
    /// Binary data.
    Blob(Cow<'a, [u8]>),
}

impl From<i64> for Param<'_> {
    fn from(v: i64) -> Self {
        Param::Int(v)
    }
}

impl From<bool> for Param<'_> {
    fn from(v: bool) -> Self {
        Param::Int(i64::from(v))
    }
}

impl<'a> From<&'a str> for Param<'a> {
    fn from(v: &'a str) -> Self {
        Param::Text(Cow::Borrowed(v))
    }
}

impl<'a> From<&'a String> for Param<'a> {
    fn from(v: &'a String) -> Self {
        Param::Text(Cow::Borrowed(v.as_str()))
    }
}

impl From<String> for Param<'_> {
    fn from(v: String) -> Self {
        Param::Text(Cow::Owned(v))
    }
}

impl<'a> From<&'a [u8]> for Param<'a> {
    fn from(v: &'a [u8]) -> Self {
        Param::Blob(Cow::Borrowed(v))
    }
}

impl From<Vec<u8>> for Param<'_> {
    fn from(v: Vec<u8>) -> Self {
        Param::Blob(Cow::Owned(v))
    }
}

impl<'a, T: Into<Param<'a>>> From<Option<T>> for Param<'a> {
    fn from(v: Option<T>) -> Self {
        v.map_or(Param::Null, Into::into)
    }
}

/// Builds a `[Param; N]` from heterogeneous values.
macro_rules! params {
    ($($e:expr),+ $(,)?) => { [$($crate::meta::db::Param::from($e)),+] };
}
pub(crate) use params;

/// One value of a result row.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// A 64-bit integer.
    Int(i64),
    /// Text.
    Text(String),
    /// Binary data.
    Blob(Vec<u8>),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Value::Null => "NULL",
            Value::Int(_) => "an integer",
            Value::Text(_) => "text",
            Value::Blob(_) => "a blob",
        }
    }
}

/// One result row with typed getters (columns are addressed by position).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(Vec<Value>);

impl Row {
    /// Wraps the values of a row.
    pub fn new(values: Vec<Value>) -> Self {
        Self(values)
    }

    /// The values, in column order.
    pub fn values(&self) -> &[Value] {
        &self.0
    }

    fn get(&self, i: usize) -> Result<&Value, MetaError> {
        self.0.get(i).ok_or_else(|| MetaError::Corrupt(format!("result row has no column {i}")))
    }

    fn mismatch(i: usize, want: &str, got: &Value) -> MetaError {
        MetaError::Corrupt(format!("column {i}: expected {want}, found {}", got.kind()))
    }

    /// A non-NULL integer column.
    pub fn i64(&self, i: usize) -> Result<i64, MetaError> {
        match self.get(i)? {
            Value::Int(v) => Ok(*v),
            other => Err(Self::mismatch(i, "an integer", other)),
        }
    }

    /// A nullable integer column.
    pub fn opt_i64(&self, i: usize) -> Result<Option<i64>, MetaError> {
        match self.get(i)? {
            Value::Null => Ok(None),
            Value::Int(v) => Ok(Some(*v)),
            other => Err(Self::mismatch(i, "an integer or NULL", other)),
        }
    }

    /// A non-NULL text column.
    pub fn str(&self, i: usize) -> Result<&str, MetaError> {
        match self.get(i)? {
            Value::Text(v) => Ok(v),
            other => Err(Self::mismatch(i, "text", other)),
        }
    }

    /// A nullable text column.
    pub fn opt_str(&self, i: usize) -> Result<Option<&str>, MetaError> {
        match self.get(i)? {
            Value::Null => Ok(None),
            Value::Text(v) => Ok(Some(v)),
            other => Err(Self::mismatch(i, "text or NULL", other)),
        }
    }

    /// A non-NULL text column, owned.
    pub fn string(&self, i: usize) -> Result<String, MetaError> {
        self.str(i).map(str::to_owned)
    }

    /// A nullable text column, owned.
    pub fn opt_string(&self, i: usize) -> Result<Option<String>, MetaError> {
        self.opt_str(i).map(|o| o.map(str::to_owned))
    }

    /// A non-NULL blob column.
    pub fn blob(&self, i: usize) -> Result<&[u8], MetaError> {
        match self.get(i)? {
            Value::Blob(v) => Ok(v),
            other => Err(Self::mismatch(i, "a blob", other)),
        }
    }
}

/// A database connection, inside whatever transaction its owner opened.
///
/// Methods take `&self`: a connection is used by one thread at a time (the
/// closure of [`super::Meta::read`] / [`super::Meta::write`]), and the
/// implementations use interior mutability where their driver needs `&mut`.
pub(crate) trait Conn {
    /// The dialect of this connection.
    fn dialect(&self) -> Dialect;

    /// Executes a statement and returns the number of affected rows.
    fn execute(&self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError>;

    /// Runs a statement and returns all its rows.
    fn query(&self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError>;

    /// The first row, if any.
    fn query_opt(&self, sql: &str, params: &[Param<'_>]) -> Result<Option<Row>, MetaError> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// The first row; its absence is a corruption of the expectation.
    fn query_one(&self, sql: &str, params: &[Param<'_>]) -> Result<Row, MetaError> {
        self.query_opt(sql, params)?.ok_or_else(|| MetaError::Corrupt(format!("statement returned no row: {sql}")))
    }
}

/// Rewrites `?1`-style placeholders to PostgreSQL's `$1`. Quoted strings and
/// quoted identifiers are left alone.
pub(crate) fn pg_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                out.push(c);
            }
            (Some(_), c) => out.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                out.push(c);
            }
            (None, '?') if chars.peek().is_some_and(char::is_ascii_digit) => out.push('$'),
            (None, c) => out.push(c),
        }
    }
    out
}

/// The SQLite connection adapter.
pub(crate) struct SqliteConn<'a>(pub &'a rusqlite::Connection);

impl rusqlite::ToSql for Param<'_> {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value as V, ValueRef};
        Ok(match self {
            Param::Null => ToSqlOutput::Owned(V::Null),
            Param::Int(i) => ToSqlOutput::Owned(V::Integer(*i)),
            Param::Text(t) => ToSqlOutput::Borrowed(ValueRef::Text(t.as_bytes())),
            Param::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

impl Conn for SqliteConn<'_> {
    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    fn execute(&self, sql: &str, params: &[Param<'_>]) -> Result<u64, MetaError> {
        let mut stmt = self.0.prepare_cached(sql)?;
        Ok(stmt.execute(rusqlite::params_from_iter(params))? as u64)
    }

    fn query(&self, sql: &str, params: &[Param<'_>]) -> Result<Vec<Row>, MetaError> {
        use rusqlite::types::ValueRef;
        let mut stmt = self.0.prepare_cached(sql)?;
        let columns = stmt.column_count();
        let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let mut values = Vec::with_capacity(columns);
            for i in 0..columns {
                values.push(match r.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => Value::Int(v),
                    ValueRef::Text(t) => Value::Text(
                        String::from_utf8(t.to_vec())
                            .map_err(|_| MetaError::Corrupt(format!("column {i} is not valid UTF-8")))?,
                    ),
                    ValueRef::Blob(b) => Value::Blob(b.to_vec()),
                    ValueRef::Real(_) => return Err(MetaError::Corrupt(format!("column {i} holds a real number"))),
                });
            }
            out.push(Row(values));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_rewritten_outside_quotes_only() {
        assert_eq!(
            pg_placeholders("SELECT a FROM t WHERE x = ?1 AND y = ?12"),
            "SELECT a FROM t WHERE x = $1 AND y = $12"
        );
        assert_eq!(pg_placeholders("SELECT '?1', \"?2\", ?3"), "SELECT '?1', \"?2\", $3");
        assert_eq!(pg_placeholders("SELECT 'it''s ?1', ?1"), "SELECT 'it''s ?1', $1");
        assert_eq!(pg_placeholders("LIKE ?1 ESCAPE '\\'"), "LIKE $1 ESCAPE '\\'");
    }

    #[test]
    fn params_convert_from_common_types() {
        let s = String::from("x");
        let p = params![1i64, "a", &s, None::<&str>, Some(5i64), true, vec![1u8, 2], s.clone()];
        assert_eq!(
            p.to_vec(),
            vec![
                Param::Int(1),
                Param::Text("a".into()),
                Param::Text("x".into()),
                Param::Null,
                Param::Int(5),
                Param::Int(1),
                Param::Blob(vec![1, 2].into()),
                Param::Text("x".into()),
            ]
        );
    }

    #[test]
    fn rows_have_typed_getters_and_report_mismatches() {
        let r = Row::new(vec![Value::Int(7), Value::Text("t".into()), Value::Null, Value::Blob(vec![1])]);
        assert_eq!(r.i64(0).unwrap(), 7);
        assert_eq!(r.str(1).unwrap(), "t");
        assert_eq!(r.opt_str(2).unwrap(), None);
        assert_eq!(r.opt_i64(2).unwrap(), None);
        assert_eq!(r.blob(3).unwrap(), &[1]);
        assert!(r.i64(1).is_err());
        assert!(r.str(2).is_err(), "NULL is not text");
        assert!(r.i64(9).is_err(), "out of range");
    }

    #[test]
    fn sqlite_adapter_round_trips_every_type() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        let conn = SqliteConn(&c);
        conn.execute("CREATE TABLE t (i INTEGER, s TEXT, b BLOB, n TEXT)", &[]).unwrap();
        let n = conn
            .execute("INSERT INTO t VALUES (?1, ?2, ?3, ?4)", &params![-5i64, "héllo", vec![0u8, 255], None::<&str>])
            .unwrap();
        assert_eq!(n, 1);
        let row = conn.query_one("SELECT i, s, b, n FROM t WHERE i = ?1", &params![-5i64]).unwrap();
        assert_eq!(row.i64(0).unwrap(), -5);
        assert_eq!(row.str(1).unwrap(), "héllo");
        assert_eq!(row.blob(2).unwrap(), &[0, 255]);
        assert_eq!(row.opt_str(3).unwrap(), None);
        assert!(conn.query_opt("SELECT i FROM t WHERE i = ?1", &params![1i64]).unwrap().is_none());
        // RETURNING, the portable replacement for last_insert_rowid()
        conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT)", &[]).unwrap();
        let id = conn.query_one("INSERT INTO a (v) VALUES (?1) RETURNING id", &params!["x"]).unwrap().i64(0).unwrap();
        assert_eq!(id, 1);
    }
}
