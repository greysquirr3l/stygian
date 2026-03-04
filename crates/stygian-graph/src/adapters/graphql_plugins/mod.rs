//! GraphQL target plugin implementations.
//!
//! Each file in this module implements [`GraphQlTargetPlugin`](crate::ports::graphql_plugin::GraphQlTargetPlugin)
//! for one specific GraphQL API target.
//!
//! # Available plugins
//!
//! | Module | Target | Notes |
//! |--------|--------|---------|
//! | [`generic`](graphql_plugins::generic) | Any GraphQL API | Fully configurable via builder |
//!
//! Consumer-specific plugins (e.g. Jobber) live in the consuming application,
//! not in this library.  Use [`generic::GenericGraphQlPlugin`](crate::adapters::graphql_plugins::generic::GenericGraphQlPlugin) to build them.

pub mod generic;
