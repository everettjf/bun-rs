//! `bun:sqlite` — embedded SQLite via rusqlite.
//!
//! API (subset of Bun's):
//!   const db = new Database(":memory:" | "/path/to.db" | undefined);
//!   db.run("CREATE TABLE foo (a, b)");
//!   db.exec(sql);                        // alias of run, multi-statement
//!   db.query(sql).all(...args) → rows
//!   db.query(sql).get(...args) → row | undefined
//!   db.query(sql).run(...args) → { lastInsertRowid, changes }
//!   db.prepare(sql) — alias of query
//!   db.close();
//!
//! Bound parameters are positional (`?`) or named (`@name` / `$name`),
//! passed by position (...args) or by name (single object arg).

use std::cell::RefCell;
use std::rc::Rc;

use bun_jsc::{Callback, Context, Value};
use rusqlite::types::{Value as SqlValue, ValueRef};

pub fn build<'ctx>(ctx: &'ctx Context) -> Value<'ctx> {
    // Build a JS-callable constructor by stashing a `make` fn on the
    // exports and wrapping in a class via JS so `new Database(...)` works.
    let exports_v = ctx.eval("({})", Some("[bun:sqlite]")).unwrap();
    let exports = exports_v.to_object().unwrap();

    let open_cb = Callback::new(ctx, "__bun_sqlite_open", |args| {
        let path = if args.len() >= 1 && !args.get(0).is_undefined() {
            args.get(0).to_string()
        } else {
            ":memory:".to_string()
        };
        let conn = rusqlite::Connection::open(&path).map_err(|e| e.to_string())?;
        let handle: Rc<RefCell<Option<rusqlite::Connection>>> =
            Rc::new(RefCell::new(Some(conn)));
        build_db_object(args.context(), handle)
    });
    exports
        .set_property("__bun_sqlite_open", &open_cb.value_in(ctx))
        .unwrap();
    std::mem::forget(open_cb);

    // JS-side: a thin Database class so `new Database(":memory:")` works.
    let js = ctx
        .eval(
            r#"(function(exports) {
                class Database {
                    constructor(filename) {
                        const inner = exports.__bun_sqlite_open(filename || ":memory:");
                        Object.assign(this, inner);
                    }
                }
                exports.Database = Database;
                exports.default = exports;
            })"#,
            Some("[bun:sqlite-wrap]"),
        )
        .unwrap()
        .to_object()
        .unwrap();
    let _ = js.call(None, &[exports_v]);
    exports_v
}

fn build_db_object<'ctx>(
    ctx: &'ctx Context,
    handle: Rc<RefCell<Option<rusqlite::Connection>>>,
) -> Result<Value<'ctx>, String> {
    let v = ctx.eval("({})", Some("[sqlite.Database]")).unwrap();
    let obj = v.to_object().map_err(|e| e.to_string())?;

    // run(sql, ...params) → { changes, lastInsertRowid }
    let h = handle.clone();
    bind(ctx, &obj, "run", move |args| {
        let sql = args.get(0).to_string();
        let params = collect_params(&args, 1);
        let g = h.borrow();
        let conn = g.as_ref().ok_or("database closed")?;
        let changes;
        let last_id;
        {
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            bind_params(&mut stmt, &params).map_err(|e| e.to_string())?;
            stmt.raw_execute().map_err(|e| e.to_string())?;
            changes = conn.changes() as f64;
            last_id = conn.last_insert_rowid() as f64;
        }
        let ctx = args.context();
        let r = ctx.eval("({})", Some("[run-result]")).unwrap();
        let ro = r.to_object().map_err(|e| e.to_string())?;
        ro.set_property("changes", &Value::new_number(ctx, changes)).unwrap();
        ro.set_property("lastInsertRowid", &Value::new_number(ctx, last_id)).unwrap();
        Ok(r)
    });

    // exec(sql) — run multiple statements; returns nothing.
    let h2 = handle.clone();
    bind(ctx, &obj, "exec", move |args| {
        let sql = args.get(0).to_string();
        let g = h2.borrow();
        let conn = g.as_ref().ok_or("database closed")?;
        conn.execute_batch(&sql).map_err(|e| e.to_string())?;
        Ok(Value::new_undefined(args.context()))
    });

    // query(sql) → Statement object (prepare alias).
    let h3 = handle.clone();
    bind(ctx, &obj, "query", move |args| {
        let sql = args.get(0).to_string();
        build_stmt(args.context(), h3.clone(), sql)
    });
    let h4 = handle.clone();
    bind(ctx, &obj, "prepare", move |args| {
        let sql = args.get(0).to_string();
        build_stmt(args.context(), h4.clone(), sql)
    });

    // close()
    let h5 = handle;
    bind(ctx, &obj, "close", move |args| {
        let _ = h5.borrow_mut().take();
        Ok(Value::new_undefined(args.context()))
    });

    Ok(v)
}

fn build_stmt<'ctx>(
    ctx: &'ctx Context,
    handle: Rc<RefCell<Option<rusqlite::Connection>>>,
    sql: String,
) -> Result<Value<'ctx>, String> {
    let v = ctx.eval("({})", Some("[sqlite.Statement]")).unwrap();
    let obj = v.to_object().map_err(|e| e.to_string())?;

    let h_all = handle.clone();
    let sql_all = sql.clone();
    bind(ctx, &obj, "all", move |args| {
        let g = h_all.borrow();
        let conn = g.as_ref().ok_or("database closed")?;
        let mut stmt = conn.prepare(&sql_all).map_err(|e| e.to_string())?;
        let params = collect_params(&args, 0);
        bind_params(&mut stmt, &params).map_err(|e| e.to_string())?;
        let col_count = stmt.column_count();
        let col_names: Vec<String> = (0..col_count)
            .map(|i| stmt.column_name(i).unwrap_or("").to_string())
            .collect();
        let mut rows = stmt.raw_query();
        let ctx = args.context();
        let arr_v = ctx.eval("[]", Some("[sqlite-rows]")).unwrap();
        let arr = arr_v.to_object().map_err(|e| e.to_string())?;
        let mut i = 0u32;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let row_v = row_to_js(ctx, row, &col_names);
            arr.set_property(&i.to_string(), &row_v).unwrap();
            i += 1;
        }
        arr.set_property("length", &Value::new_number(ctx, i as f64)).unwrap();
        Ok(arr_v)
    });

    let h_get = handle.clone();
    let sql_get = sql.clone();
    bind(ctx, &obj, "get", move |args| {
        let g = h_get.borrow();
        let conn = g.as_ref().ok_or("database closed")?;
        let mut stmt = conn.prepare(&sql_get).map_err(|e| e.to_string())?;
        let params = collect_params(&args, 0);
        bind_params(&mut stmt, &params).map_err(|e| e.to_string())?;
        let col_names: Vec<String> = (0..stmt.column_count())
            .map(|i| stmt.column_name(i).unwrap_or("").to_string())
            .collect();
        let mut rows = stmt.raw_query();
        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            Ok(row_to_js(args.context(), row, &col_names))
        } else {
            Ok(Value::new_undefined(args.context()))
        }
    });

    let h_run = handle.clone();
    let sql_run = sql.clone();
    bind(ctx, &obj, "run", move |args| {
        let g = h_run.borrow();
        let conn = g.as_ref().ok_or("database closed")?;
        let mut stmt = conn.prepare(&sql_run).map_err(|e| e.to_string())?;
        let params = collect_params(&args, 0);
        bind_params(&mut stmt, &params).map_err(|e| e.to_string())?;
        stmt.raw_execute().map_err(|e| e.to_string())?;
        let changes = conn.changes() as f64;
        let last_id = conn.last_insert_rowid() as f64;
        let ctx = args.context();
        let r = ctx.eval("({})", Some("[run-result]")).unwrap();
        let ro = r.to_object().map_err(|e| e.to_string())?;
        ro.set_property("changes", &Value::new_number(ctx, changes)).unwrap();
        ro.set_property("lastInsertRowid", &Value::new_number(ctx, last_id)).unwrap();
        Ok(r)
    });

    Ok(v)
}

fn row_to_js<'a>(
    ctx: &'a Context,
    row: &rusqlite::Row<'_>,
    col_names: &[String],
) -> Value<'a> {
    let v = ctx.eval("({})", Some("[sqlite-row]")).unwrap();
    let obj = v.to_object().unwrap();
    for (i, name) in col_names.iter().enumerate() {
        let cv = row.get_ref(i).unwrap_or(ValueRef::Null);
        let jv = sql_to_js(ctx, cv);
        let _ = obj.set_property(name, &jv);
    }
    v
}

fn sql_to_js<'a>(ctx: &'a Context, v: ValueRef<'_>) -> Value<'a> {
    match v {
        ValueRef::Null => Value::new_null(ctx),
        ValueRef::Integer(i) => Value::new_number(ctx, i as f64),
        ValueRef::Real(r) => Value::new_number(ctx, r),
        ValueRef::Text(t) => {
            let s = std::str::from_utf8(t).unwrap_or("");
            Value::new_string(ctx, s)
        }
        ValueRef::Blob(b) => crate::buffer::buffer_from_bytes(ctx, b.to_vec()),
    }
}

enum BoundParam {
    Positional(Vec<SqlValue>),
    Named(Vec<(String, SqlValue)>),
}

fn collect_params(args: &bun_jsc::CallbackArgs<'_>, start: usize) -> BoundParam {
    // If exactly one arg and it's a plain object (not array/buffer/null/undefined),
    // treat as named params. Otherwise positional list of remaining args.
    if args.len() == start + 1 {
        let v = args.get(start);
        if v.is_object() && !v.is_nullish() {
            // skip arrays and Uint8Array — those are positional/blob respectively
            let looks_array_or_typed = v
                .to_object()
                .ok()
                .and_then(|o| o.get_property("length").ok())
                .map(|l| l.is_number())
                .unwrap_or(false);
            if !looks_array_or_typed {
                if let Ok(obj) = v.to_object() {
                    let names = obj.property_names();
                    let mut named = Vec::with_capacity(names.len());
                    for n in names {
                        if let Ok(val) = obj.get_property(&n) {
                            named.push((n, js_to_sql(&val)));
                        }
                    }
                    return BoundParam::Named(named);
                }
            }
        }
    }
    let mut positional = Vec::with_capacity(args.len() - start);
    for i in start..args.len() {
        positional.push(js_to_sql(&args.get(i)));
    }
    BoundParam::Positional(positional)
}

fn js_to_sql(v: &Value<'_>) -> SqlValue {
    if v.is_null() || v.is_undefined() {
        return SqlValue::Null;
    }
    if v.is_boolean() {
        return SqlValue::Integer(if v.to_bool() { 1 } else { 0 });
    }
    if v.is_number() {
        let n = v.to_number();
        if n.fract() == 0.0 && n.is_finite() && n.abs() < (i64::MAX as f64) {
            return SqlValue::Integer(n as i64);
        }
        return SqlValue::Real(n);
    }
    if v.is_string() {
        return SqlValue::Text(v.to_string());
    }
    if let Some(b) = v.typed_array_bytes() {
        return SqlValue::Blob(b.to_vec());
    }
    SqlValue::Text(v.to_string())
}

fn bind_params(
    stmt: &mut rusqlite::Statement<'_>,
    params: &BoundParam,
) -> rusqlite::Result<()> {
    match params {
        BoundParam::Positional(list) => {
            for (i, v) in list.iter().enumerate() {
                stmt.raw_bind_parameter(i + 1, v)?;
            }
        }
        BoundParam::Named(list) => {
            for (name, v) in list {
                let with = format!(":{}", name);
                let alt = format!("@{}", name);
                let dollar = format!("${}", name);
                let idx = stmt
                    .parameter_index(&with)
                    .ok()
                    .flatten()
                    .or_else(|| stmt.parameter_index(&alt).ok().flatten())
                    .or_else(|| stmt.parameter_index(&dollar).ok().flatten());
                if let Some(i) = idx {
                    stmt.raw_bind_parameter(i, v)?;
                }
            }
        }
    }
    Ok(())
}

fn bind<F>(ctx: &Context, obj: &bun_jsc::Object<'_>, name: &str, f: F)
where
    F: for<'a> Fn(bun_jsc::CallbackArgs<'a>) -> Result<Value<'a>, String> + 'static,
{
    let cb = Callback::new(ctx, name, f);
    obj.set_property(name, &cb.value_in(ctx)).unwrap();
    std::mem::forget(cb);
}
