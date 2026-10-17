//! A small blocking connection pool.
//!
//! Connections are created lazily up to `pool_size`, handed out exclusively and
//! returned on drop. A connection is only returned if it is healthy: one that
//! failed with an I/O error, was dropped in the middle of a transaction (for
//! example by a panic), or is closed is discarded, which also makes the server
//! roll back whatever it was doing and release its locks.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use super::tls::Connector;
use super::url::Target;
use super::{PgClient, PgOptions, classify, quote_ident};
use crate::meta::MetaError;

struct State {
    idle: Vec<PgClient>,
    total: usize,
}

struct Shared {
    config: postgres::Config,
    connector: Connector,
    schema: String,
    opts: PgOptions,
    state: Mutex<State>,
    available: Condvar,
}

/// The pool.
pub(crate) struct Pool {
    shared: Arc<Shared>,
}

/// A connection checked out of the pool.
pub(crate) struct Pooled {
    shared: Arc<Shared>,
    client: Option<PgClient>,
}

impl Pool {
    pub(crate) fn new(target: Target, connector: Connector, schema: &str, opts: &PgOptions) -> Self {
        let mut config = target.config;
        config
            .application_name(&opts.application_name)
            .connect_timeout(opts.connect_timeout)
            .keepalives(true)
            .keepalives_idle(std::time::Duration::from_secs(60));
        Self {
            shared: Arc::new(Shared {
                config,
                connector,
                schema: schema.to_string(),
                opts: opts.clone(),
                state: Mutex::new(State { idle: Vec::new(), total: 0 }),
                available: Condvar::new(),
            }),
        }
    }

    /// A connection of its own, not part of the pool (session state such as `LISTEN` must not leak into it).
    pub(crate) fn connect_dedicated(&self) -> Result<postgres::Client, MetaError> {
        self.shared.connect_raw()
    }

    pub(crate) fn total(&self) -> usize {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).total
    }

    /// Checks a connection out, opening one if the pool has room, otherwise
    /// waiting up to `checkout_timeout`.
    pub(crate) fn get(&self) -> Result<Pooled, MetaError> {
        let s = &self.shared;
        let deadline = Instant::now() + s.opts.checkout_timeout;
        let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(c) = st.idle.pop() {
                if c.client.is_closed() {
                    st.total -= 1;
                    s.available.notify_one();
                    continue;
                }
                return Ok(Pooled { shared: s.clone(), client: Some(c) });
            }
            if st.total < s.opts.pool_size {
                st.total += 1;
                drop(st);
                return match s.connect() {
                    Ok(c) => Ok(Pooled { shared: s.clone(), client: Some(c) }),
                    Err(e) => {
                        s.state.lock().unwrap_or_else(|e| e.into_inner()).total -= 1;
                        s.available.notify_one();
                        Err(e)
                    }
                };
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(MetaError::Retriable(format!(
                    "timed out after {:?} waiting for a free database connection (pool of {})",
                    s.opts.checkout_timeout, s.opts.pool_size
                )));
            }
            st = s.available.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Shared {
    fn connect_raw(&self) -> Result<postgres::Client, MetaError> {
        match &self.connector {
            Connector::Plain => self.config.connect(postgres::NoTls),
            #[cfg(feature = "tls")]
            Connector::Rustls(m) => self.config.connect(m.clone()),
        }
        .map_err(|e| classify(&e, false))
    }

    fn connect(&self) -> Result<PgClient, MetaError> {
        let client = self.connect_raw()?;
        let mut c = PgClient { client, stmts: HashMap::new(), in_tx: false, broken: false, last_used: Instant::now() };
        let mut setup = format!(
            "SET search_path TO {}; SET idle_in_transaction_session_timeout = {}",
            quote_ident(&self.schema),
            super::ms(self.opts.idle_in_transaction_timeout)
        );
        if let Some(t) = self.opts.statement_timeout {
            setup.push_str(&format!("; SET statement_timeout = {}", super::ms(t)));
        }
        c.batch(&setup)?;
        Ok(c)
    }
}

impl Pooled {
    pub(crate) fn is_dead(&self) -> bool {
        self.client().is_dead()
    }

    pub(super) fn client(&self) -> &PgClient {
        self.client.as_ref().expect("connection present until drop")
    }

    pub(super) fn client_mut(&mut self) -> &mut PgClient {
        self.client.as_mut().expect("connection present until drop")
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        let Some(mut c) = self.client.take() else { return };
        let healthy = !c.broken && !c.in_tx && !c.client.is_closed();
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if healthy {
            c.last_used = Instant::now();
            st.idle.push(c);
        } else {
            st.total -= 1;
            drop(st);
            drop(c);
        }
        self.shared.available.notify_one();
    }
}
