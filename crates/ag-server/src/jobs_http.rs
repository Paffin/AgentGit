//! HTTP routes of the execution plane (jobs).

use axum::Router;

use crate::app::App;
use crate::routes::RouteMeta;

/// Adds the job routes.
pub fn add_routes(_r: &mut Router<App>, _meta: &mut Vec<RouteMeta>) {}
